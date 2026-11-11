//! Portable capture integrity and finite controls. No production dependencies.
#![forbid(unsafe_code)]
use std::{collections::BTreeMap, env, fs, path::Path, process};

const N: usize = 64;
const G: usize = 256;
const BLOCK: usize = 2 * G + 64 * N;
const HALF: [u16; 17] = [
    0, 0x2c00, 0x3000, 0x3200, 0x3400, 0x3500, 0x3600, 0x3700, 0x3800, 0x3880, 0x3900, 0x3980,
    0x3a00, 0x3a80, 0x3b00, 0x3b80, 0x3c00,
];
type Result<T> = std::result::Result<T, String>;
fn need(b: bool, s: &str) -> Result<()> {
    if b {
        Ok(())
    } else {
        Err(s.into())
    }
}
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|_| "file_io".into())
}
fn dims(t: usize) -> (usize, usize, usize) {
    if t == 0 {
        (7, 5, 256)
    } else {
        (11, 3, 512)
    }
}
fn seed(x: usize, y: usize) -> [usize; 4] {
    [x, y, (3 * x + 5 * y) % 16, 16]
}
fn input(t: usize) -> Vec<u8> {
    let (w, h, pitch) = dims(t);
    let mut b = vec![0xa5; 2 * G + pitch * h];
    for y in 0..h {
        for x in 0..w {
            for (c, v) in seed(x, y).iter().enumerate() {
                let at = G + y * pitch + x * 8 + c * 2;
                b[at..at + 2].copy_from_slice(&HALF[*v].to_le_bytes());
            }
        }
    }
    b
}
fn coords(t: usize) -> Vec<u8> {
    let (w, h, _) = dims(t);
    let mut xy = vec![
        (0.5, 0.5),
        (w as f32 / 2.0, h as f32 / 2.0),
        (-2.0, -3.0),
        (w as f32 + 2.0, h as f32 + 3.0),
        (1.75, 1.75),
        (1.0, 1.0),
        (-0.0, 0.0),
        (w as f32, h as f32),
    ];
    for y in 0..h {
        for x in 0..w {
            xy.push((x as f32 + 0.5, y as f32 + 0.5));
        }
    }
    for base in [0.5_f32, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0] {
        for step in [-1_i32, 0, 1] {
            xy.push((
                f32::from_bits(base.to_bits().wrapping_add_signed(step)),
                1.5,
            ));
        }
    }
    xy.truncate(N);
    assert_eq!(xy.len(), N);
    xy.into_iter()
        .flat_map(|(x, y)| [x, y, x / w as f32, y / h as f32])
        .flat_map(|f| f.to_bits().to_le_bytes())
        .collect()
}
fn write_new(root: &Path, name: &str, b: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = io(fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name)))?;
    io(f.write_all(b))
}
fn prepare(root: &Path) -> Result<()> {
    for t in 0..2 {
        write_new(root, &format!("input{t}.bin"), &input(t))?;
        write_new(root, &format!("coords{t}.bin"), &coords(t))?;
    }
    Ok(())
}
fn bounds(root: &Path) -> Result<()> {
    need(
        io(fs::symlink_metadata(root))?.file_type().is_dir(),
        "root_type",
    )?;
    let mut bytes = 0;
    let mut count = 0;
    for entry in io(fs::read_dir(root))? {
        let e = io(entry)?;
        let m = io(fs::symlink_metadata(e.path()))?;
        need(m.file_type().is_file(), "not_plain_file")?;
        let n = e.file_name().into_string().map_err(|_| "file_name")?;
        let mut cap = 0;
        for t in 0..2 {
            if n == format!("input{t}.bin") {
                cap = input(t).len();
            }
            if n == format!("coords{t}.bin") {
                cap = N * 16;
            }
            if n == format!("transfer{t}.bin") {
                cap = input(t).len() * 2;
            }
        }
        if (0..24).any(|id| n == format!("case{id:02}.bin")) {
            cap = BLOCK * 2;
        }
        if [
            "cases.jsonl",
            "environment.json",
            "completion.json",
            "check.json",
            "negative.json",
            "baseline.txt",
            "source-sha256.txt",
            "command-exit.json",
            "SHA256SUMS",
        ]
        .contains(&n.as_str())
        {
            cap = 65536;
        }
        need(
            cap > 0 && m.len() <= cap as u64,
            "artifact_allowlist_or_size",
        )?;
        count += 1;
        bytes += m.len();
    }
    need(count <= 40 && bytes <= 2 * 1024 * 1024, "artifact_budget")
}
// Deliberately narrow JSON object parser: flat known numeric/Boolean receipts only.
// It is not used to interpret arbitrary native environment/error strings.
fn numbers(s: &str) -> Result<BTreeMap<String, u64>> {
    let s = s
        .trim()
        .strip_prefix('{')
        .and_then(|x| x.strip_suffix('}'))
        .ok_or("json_object")?;
    let mut map = BTreeMap::new();
    for item in s.split(',') {
        let (key, val) = item.split_once(':').ok_or("json_field")?;
        let key = key
            .trim()
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .ok_or("json_key")?;
        need(
            !key.is_empty() && key.bytes().all(|c| c.is_ascii_lowercase() || c == b'_'),
            "json_key_chars",
        )?;
        let v = match val.trim() {
            "true" => 1,
            "false" => 0,
            x => x.parse().map_err(|_| "json_number")?,
        };
        need(map.insert(key.to_string(), v).is_none(), "json_duplicate")?;
    }
    Ok(map)
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn half(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn widen(h: u16) -> f32 {
    let e = (h >> 10) & 31;
    let m = h & 1023;
    let value = if e == 0 {
        m as f32 / 16777216.0
    } else {
        (1.0 + m as f32 / 1024.0) * 2.0_f32.powi(e as i32 - 15)
    };
    if h & 0x8000 != 0 {
        -value
    } else {
        value
    }
}
fn hash(b: &[u8]) -> u32 {
    (0..48)
        .step_by(4)
        .fold(2166136261, |v, i| (v ^ word(b, i)).wrapping_mul(16777619))
}
fn raw_check(b: &[u8], id: usize) -> Result<u64> {
    need(b.len() == BLOCK * 2, "raw_size")?;
    let (w, h, _) = dims(id / 12);
    let mut half_float_different = 0;
    for stage in 0..2 {
        let a = &b[stage * BLOCK..(stage + 1) * BLOCK];
        need(
            a[..G]
                .iter()
                .chain(a[G + N * 64..].iter())
                .all(|x| *x == 0xa5),
            "raw_guard",
        )?;
        for i in 0..N {
            let r = &a[G + i * 64..G + (i + 1) * 64];
            need(
                word(r, 48) == id as u32
                    && word(r, 52) == i as u32
                    && word(r, 56) == stage as u32
                    && word(r, 60) == hash(r),
                "raw_marker_or_hash",
            )?;
            need(r[24..32] == [0; 8], "row_reserved")?;
            need(
                [word(r, 32), word(r, 36), word(r, 40), word(r, 44)]
                    == [w as u32, h as u32, w as u32, h as u32],
                "uint_mip_zero_query",
            )?;
            for c in 0..4 {
                let f = f32::from_bits(word(r, c * 4));
                let hb = half(r, 16 + c * 2);
                need(
                    f.is_finite() && (0.0..=1.0).contains(&f) && hb <= 0x3c00,
                    "finite_range",
                )?;
                if word(r, c * 4) != widen(hb).to_bits() {
                    half_float_different += 1;
                }
            }
        }
    }
    if (id / 6) % 2 == 0 {
        need(half_float_different == 0, "nearest_half_float_return")?;
    }
    Ok(half_float_different)
}
fn row(b: &[u8], stage: usize, i: usize) -> &[u8] {
    &b[stage * BLOCK + G + i * 64..stage * BLOCK + G + (i + 1) * 64]
}
fn anchor(r: &[u8], x: usize, y: usize, wrong: bool) -> Result<()> {
    for (c, v) in seed(x, y).iter().enumerate() {
        let mut expected = (*v as f32 / 16.0).to_bits();
        if wrong && c == 0 {
            expected ^= 1;
        }
        need(
            word(r, c * 4) == expected && half(r, 16 + c * 2) == HALF[*v],
            "oracle_anchor_mismatch",
        )?;
    }
    Ok(())
}
fn receipt(s: &str, id: usize) -> Result<()> {
    let (w, h, _) = dims(id / 12);
    let mode = (id / 2) % 3;
    let f = (id / 6) % 2;
    let expected: BTreeMap<String, u64> = [
        ("id", id),
        ("texture", id / 12),
        ("filter", f),
        ("mode", mode),
        ("avg", id % 2),
        ("reported", id % 2),
        ("setter", 1),
        ("getter", 1),
        ("abi", 1),
        ("min", f),
        ("mag", f),
        ("mip", 0),
        ("normalized", usize::from(mode != 0)),
        ("s", 0),
        ("t", 0),
        ("r", 0),
        ("anisotropy", 1),
        ("compare", 0),
        ("lod_min_bits", 0),
        ("lod_max_bits", 0x7f7fffff),
        ("width", w),
        ("height", h),
        ("mips", 1),
        ("samples", 1),
        ("status", 4),
        ("error", 0),
        ("commit", 1),
        ("wait", 1),
        ("bytes", 2 * BLOCK),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v as u64))
    .collect();
    need(numbers(s)? == expected, "actual_api_receipt")
}
fn verify(root: &Path, wrong: bool) -> Result<(u64, u64)> {
    bounds(root)?;
    let complete = io(fs::read_to_string(root.join("completion.json")))?;
    need(
        numbers(&complete)? == numbers("{\"cases\":24,\"transfers\":2,\"status\":1}")?,
        "completion",
    )?;
    need(
        io(fs::metadata(root.join("environment.json")))?.len() > 0,
        "environment_missing",
    )?;
    for t in 0..2 {
        let expected = input(t);
        need(
            io(fs::read(root.join(format!("input{t}.bin"))))? == expected,
            "input_identity",
        )?;
        need(
            io(fs::read(root.join(format!("coords{t}.bin"))))? == coords(t),
            "coordinate_identity",
        )?;
        let transfer = io(fs::read(root.join(format!("transfer{t}.bin"))))?;
        need(
            transfer == [expected.as_slice(), expected.as_slice()].concat(),
            "texture_roundtrip",
        )?;
    }
    let receipts = io(fs::read_to_string(root.join("cases.jsonl")))?;
    need(receipts.lines().count() == 24, "receipt_count")?;
    let mut captures = Vec::new();
    let mut half_float = 0;
    for (id, line) in receipts.lines().enumerate() {
        receipt(line, id)?;
        let b = io(fs::read(root.join(format!("case{id:02}.bin"))))?;
        half_float += raw_check(&b, id)?;
        let (w, h, _) = dims(id / 12);
        let mode = (id / 2) % 3;
        for stage in 0..2 {
            if mode == 0 {
                anchor(row(&b, stage, 0), 0, 0, wrong && id == 0 && stage == 0)?;
                for y in 0..h {
                    for x in 0..w {
                        anchor(row(&b, stage, 8 + y * w + x), x, y, false)?;
                    }
                }
                if (id / 6) % 2 == 0 {
                    anchor(row(&b, stage, 4), 1, 1, false)?;
                } else {
                    let r = row(&b, stage, 4);
                    for c in 0..2 {
                        let f = f32::from_bits(word(r, c * 4));
                        let h = widen(half(r, 16 + c * 2));
                        need(
                            f > 1.0 / 16.0 && f < 2.0 / 16.0 && h > 1.0 / 16.0 && h < 2.0 / 16.0,
                            "linear_fractional_control",
                        )?;
                    }
                }
            }
            if mode == 2 {
                anchor(row(&b, stage, 0), w / 2, h / 2, false)?;
            }
            if mode != 2 {
                anchor(row(&b, stage, 1), w / 2, h / 2, false)?;
            }
            anchor(row(&b, stage, 2), 0, 0, false)?;
            anchor(row(&b, stage, 3), w - 1, h - 1, false)?;
        }
        for i in 0..N {
            need(
                row(&b, 0, i)[..48] == row(&b, 1, i)[..48],
                "explicit_implicit_difference",
            )?;
        }
        captures.push(b);
    }
    for id in (0..24).step_by(2) {
        for stage in 0..2 {
            for i in 0..N {
                need(
                    row(&captures[id], stage, i)[..48] == row(&captures[id + 1], stage, i)[..48],
                    "lod_average_difference",
                )?;
            }
        }
    }
    let mut coordinate_differences = 0;
    for base in [0, 6, 12, 18] {
        for avg in 0..2 {
            for stage in 0..2 {
                need(
                    row(&captures[base + avg], stage, 0)[..24]
                        != row(&captures[base + 4 + avg], stage, 0)[..24],
                    "wrong_normalizedness_not_discriminating",
                )?;
                for i in 0..N {
                    if row(&captures[base + avg], stage, i)[..24]
                        != row(&captures[base + 2 + avg], stage, i)[..24]
                    {
                        coordinate_differences += 1;
                    }
                }
            }
        }
    }
    Ok((half_float, coordinate_differences))
}
fn run() -> Result<i32> {
    let a: Vec<_> = env::args().collect();
    need(a.len() == 3, "usage_command_directory")?;
    let root = Path::new(&a[2]);
    match a[1].as_str() {
        "prepare" => prepare(root)?,
        "bounds" => bounds(root)?,
        "verify" => {
            let (h, c) = verify(root, false)?;
            println!(
                "FINITE_CONTROLS_ONLY half_float_differences={h} pixel_normalized_differences={c}"
            );
        }
        "check" => {
            let (h, c) = verify(root, false)?;
            write_new(root, "check.json", format!("{{\"finite_controls_passed\":true,\"cases\":24,\"half_float_lane_differences\":{h},\"pixel_normalized_row_differences\":{c},\"general_filter_model_qualified\":false,\"air_flags_qualified\":false}}\n").as_bytes())?;
        }
        "negative" => {
            verify(root, false)?; // A broken acquisition cannot satisfy the negative.
            need(
                verify(root, true) == Err("oracle_anchor_mismatch".into()),
                "wrong_expected_negative_not_rejected",
            )?;
            write_new(root, "negative.json", b"{\"healthy_baseline_rechecked\":true,\"mismatched_oracle_expected_rejected\":true}\n")?;
            return Ok(2);
        }
        _ => return Err("unknown_command".into()),
    }
    Ok(0)
}
fn main() {
    match run() {
        Ok(code) => process::exit(code),
        Err(e) => {
            eprintln!("NOT_PROOF {e}");
            process::exit(3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Arbitrary host protocol fixture, NEVER saved or presented as GPU evidence.
    fn fake(id: usize) -> Vec<u8> {
        let mut b = vec![0xa5; 2 * BLOCK];
        let (w, h, _) = dims(id / 12);
        for stage in 0..2 {
            for i in 0..N {
                let r = &mut b[stage * BLOCK + G + i * 64..stage * BLOCK + G + (i + 1) * 64];
                r.fill(0);
                for (at, value) in [
                    (32, w as u32),
                    (36, h as u32),
                    (40, w as u32),
                    (44, h as u32),
                    (48, id as u32),
                    (52, i as u32),
                    (56, stage as u32),
                ] {
                    r[at..at + 4].copy_from_slice(&value.to_le_bytes());
                }
                let v = hash(r);
                r[60..64].copy_from_slice(&v.to_le_bytes());
            }
        }
        b
    }
    #[test]
    fn input_bounds() {
        for t in 0..2 {
            assert_eq!(coords(t).len(), N * 16);
            assert_eq!(input(t)[0], 0xa5);
        }
    }
    #[test]
    fn half_table() {
        for (i, h) in HALF.iter().enumerate() {
            assert_eq!(widen(*h), i as f32 / 16.0);
        }
    }
    #[test]
    fn protocol_only_valid() {
        for id in 0..24 {
            assert_eq!(raw_check(&fake(id), id), Ok(0));
        }
    }
    #[test]
    fn short_rejected() {
        assert_eq!(
            raw_check(&fake(0)[..2 * BLOCK - 1], 0),
            Err("raw_size".into())
        );
    }
    #[test]
    fn all_guard_mutations_rejected() {
        for at in [0, G - 1, BLOCK - G, BLOCK - 1, BLOCK, 2 * BLOCK - 1] {
            let mut b = fake(0);
            b[at] ^= 1;
            assert_eq!(raw_check(&b, 0), Err("raw_guard".into()));
        }
    }
    #[test]
    fn all_row_word_mutations_rejected() {
        for stage in 0..2 {
            for word in 0..16 {
                let mut b = fake(0);
                b[stage * BLOCK + G + word * 4] ^= 1;
                assert!(raw_check(&b, 0).is_err());
            }
        }
    }
    #[test]
    fn wrong_query_with_valid_hash_rejected() {
        let mut b = fake(0);
        b[G + 32] ^= 1;
        let v = hash(&b[G..G + 64]);
        b[G + 60..G + 64].copy_from_slice(&v.to_le_bytes());
        assert_eq!(raw_check(&b, 0), Err("uint_mip_zero_query".into()));
    }
    #[test]
    fn poison_rejected() {
        assert!(raw_check(&vec![0xa5; 2 * BLOCK], 0).is_err());
    }
    #[test]
    fn json_duplicate_rejected() {
        assert!(numbers("{\"x\":1,\"x\":2}").is_err());
    }
    #[test]
    fn json_bad_type_rejected() {
        for s in ["{}", "{\"x\":null}", "{\"x\":-1}", "{\"x\":[]}"] {
            assert!(numbers(s).is_err());
        }
    }
    #[test]
    fn bool_receipt() {
        assert_eq!(numbers("{\"x\":true,\"y\":false}").unwrap()["x"], 1);
    }
    #[test]
    fn expected_negative_is_exact() {
        let mut r = [0_u8; 64];
        for (c, v) in seed(0, 0).iter().enumerate() {
            r[c * 4..c * 4 + 4].copy_from_slice(&(*v as f32 / 16.0).to_bits().to_le_bytes());
            r[16 + c * 2..18 + c * 2].copy_from_slice(&HALF[*v].to_le_bytes());
        }
        assert_eq!(anchor(&r, 0, 0, false), Ok(()));
        assert_eq!(anchor(&r, 0, 0, true), Err("oracle_anchor_mismatch".into()));
    }
    #[test]
    fn receipt_fields_are_all_load_bearing() {
        let s = "{\"id\":0,\"texture\":0,\"filter\":0,\"mode\":0,\"avg\":0,\"reported\":0,\"setter\":1,\"getter\":1,\"abi\":1,\"min\":0,\"mag\":0,\"mip\":0,\"normalized\":0,\"s\":0,\"t\":0,\"r\":0,\"anisotropy\":1,\"compare\":0,\"lod_min_bits\":0,\"lod_max_bits\":2139095039,\"width\":7,\"height\":5,\"mips\":1,\"samples\":1,\"status\":4,\"error\":0,\"commit\":1,\"wait\":1,\"bytes\":9216}";
        assert_eq!(receipt(s, 0), Ok(()));
        let map = numbers(s).unwrap();
        assert_eq!(map.len(), 29);
        for key in map.keys() {
            let mut changed = map.clone();
            *changed.get_mut(key).unwrap() ^= 1;
            let mutated = format!(
                "{{{}}}",
                changed
                    .iter()
                    .map(|(k, v)| format!("\"{k}\":{v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            assert_eq!(receipt(&mutated, 0), Err("actual_api_receipt".into()));
        }
        assert!(receipt(&s.replacen("\"id\":0,", "", 1), 0).is_err());
        assert!(receipt(&s.replacen('{', "{\"unknown\":0,", 1), 0).is_err());
    }
    #[test]
    fn query_height_and_half_queries_are_load_bearing() {
        for at in [36, 40, 44] {
            let mut b = fake(0);
            b[G + at] ^= 1;
            let v = hash(&b[G..G + 64]);
            b[G + 60..G + 64].copy_from_slice(&v.to_le_bytes());
            assert_eq!(raw_check(&b, 0), Err("uint_mip_zero_query".into()));
        }
    }
}
