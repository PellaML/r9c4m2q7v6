//! Standalone synthetic input generator / acquisition checker, NOT a raster comparator.
//! No production dependencies, helper simulation, network, packages, or native fixtures.
#![forbid(unsafe_code)]
use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::Path,
    process::{self, Command},
};

type Result<T> = std::result::Result<T, String>;
const N: usize = 4096;
const G: usize = 256;
const RECORD: usize = 96;
const LIMIT: u64 = 4 * 1024 * 1024;
const MAGIC: u32 = 0x48464f31;
const SOURCE: [&str; 5] = [
    ".github/workflows/fetch-half.yml",
    "scripts/fetch-edge/half.metal",
    "scripts/fetch-edge/half.swift",
    "scripts/fetch-edge/half-check.rs",
    "scripts/fetch-edge/half.sh",
];
const HEADER: &str = "case\tformat\tseed\tsplit\tdraw\trx\try\trw\trh\tsx\tsy\tsw\tsh\tdiagonal\twinding\tmutation\tphase\tscale_bits\tsnapshot\n";
const ROUND: [u32; 32] = [
    0x3f800000, 0xbf800000, 0x3f800fff, 0x3f801000, 0x3f801001, 0x3f803000, 0xbf800fff, 0xbf801000,
    0xbf801001, 0xbf803000, 0x3f000fff, 0x3f001000, 0x3f001001, 0xbf001000, 0x40001000, 0xc0001000,
    0x38800000, 0xb8800000, 0x38801000, 0xb8801000, 0x33800000, 0xb3800000, 0x33000000, 0xb3000000,
    0x387fc000, 0xb87fc000, 0x00000000, 0x80000000, 0x33000001, 0xb3000001, 0x00800000, 0x80800000,
];
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
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn half(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn low(w: u32) -> u16 {
    w as u16
}
fn high(w: u32) -> u16 {
    (w >> 16) as u16
}
fn finite(h: u16) -> bool {
    h & 0x7c00 != 0x7c00
}
fn subnormal(h: u16) -> bool {
    h & 0x7c00 == 0 && h & 0x03ff != 0
}
fn normal(h: u16) -> bool {
    h & 0x7c00 != 0 && finite(h)
}
fn plain_read(root: &Path, name: &str, cap: usize) -> Result<Vec<u8>> {
    let m = io(fs::symlink_metadata(root.join(name)))?;
    need(
        m.file_type().is_file() && m.len() <= cap as u64,
        "plain_file_size",
    )?;
    io(fs::read(root.join(name)))
}
fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut f = io(fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name)))?;
    io(f.write_all(bytes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Draw {
    rect: [u32; 4],
    sc: [u32; 4],
    diag: u32,
    wind: u32,
    mode: u32,
    phase: u32,
    scale: u16,
}
#[derive(Clone, Debug)]
struct Case {
    format: usize,
    seed: usize,
    split: bool,
    draws: Vec<Draw>,
}
fn draw(rect: [u32; 4], sc: [u32; 4], diag: u32, wind: u32, mode: u32) -> Draw {
    Draw {
        rect,
        sc,
        diag,
        wind,
        mode,
        phase: 0,
        scale: 0x3c00,
    }
}
fn cases() -> Vec<Case> {
    let full = [0, 0, 64, 64];
    let small = [3, 5, 18, 15];
    let mut v = Vec::new();
    for mode in 0..2 {
        v.push(Case {
            format: 0,
            seed: 2,
            split: false,
            draws: vec![draw(full, full, 0, 0, mode)],
        });
    }
    for diag in 0..2 {
        for wind in 0..2 {
            for mode in 0..2 {
                v.push(Case {
                    format: 0,
                    seed: 0,
                    split: false,
                    draws: vec![draw(small, full, diag, wind, mode)],
                });
            }
        }
    }
    for sc in [[22, 24, 18, 14], [21, 23, 19, 15]] {
        for diag in 0..2 {
            for mode in 0..2 {
                v.push(Case {
                    format: 0,
                    seed: 0,
                    split: false,
                    draws: vec![draw(full, sc, diag, diag, mode)],
                });
            }
        }
    }
    let seq: Vec<_> = (0..3)
        .map(|phase| {
            let mut d = draw(small, full, 1, 0, 1);
            d.phase = phase;
            d.scale = 0x3e00;
            d
        })
        .collect();
    v.push(Case {
        format: 0,
        seed: 0,
        split: false,
        draws: seq.clone(),
    });
    let mut a = draw(full, [0, 0, 31, 64], 0, 1, 1);
    a.scale = 0x3800;
    let mut b = draw(small, full, 1, 1, 1);
    b.phase = 1;
    b.scale = 0x3e00;
    v.push(Case {
        format: 0,
        seed: 0,
        split: false,
        draws: vec![a, b],
    });
    v.push(Case {
        format: 0,
        seed: 0,
        split: true,
        draws: seq,
    });
    let mut a = draw(small, full, 0, 1, 0);
    a.scale = 0x3800;
    let mut b = a;
    b.mode = 1;
    b.phase = 1;
    v.push(Case {
        format: 0,
        seed: 1,
        split: false,
        draws: vec![a, b],
    });
    for mode in 0..2 {
        v.push(Case {
            format: 1,
            seed: 3,
            split: false,
            draws: vec![draw(small, full, 1, 1, mode)],
        });
    }
    assert_eq!(v.len(), 24);
    v
}
fn inside(r: [u32; 4], x: usize, y: usize) -> bool {
    x >= r[0] as usize
        && y >= r[1] as usize
        && x < (r[0] + r[2]) as usize
        && y < (r[1] + r[3]) as usize
}
fn covered(d: Draw, pixel: usize) -> bool {
    inside(d.rect, pixel % 64, pixel / 64) && inside(d.sc, pixel % 64, pixel / 64)
}
fn count(c: &Case) -> usize {
    c.draws
        .iter()
        .map(|d| (0..N).filter(|p| covered(*d, *p)).count())
        .sum()
}
// Integer geometry ONLY selects strict diagnostic witnesses. It never predicts
// native top-left tie ownership, derivative values, or unobserved helper values.
fn side(d: Draw, pixel: usize) -> i32 {
    let xx = 2 * (pixel % 64) as i32 + 1 - 2 * d.rect[0] as i32;
    let yy = 2 * (pixel / 64) as i32 + 1 - 2 * d.rect[1] as i32;
    let w = d.rect[2] as i32;
    let h = d.rect[3] as i32;
    if d.diag == 0 {
        xx * h + yy * w - 2 * w * h
    } else {
        yy * w - xx * h
    }
}
fn strict_quad(d: Draw, pixel: usize) -> bool {
    let q = (pixel / 64 & !1) * 64 + (pixel % 64 & !1);
    [q, q + 1, q + 64, q + 65]
        .into_iter()
        .all(|p| side(d, p) != 0)
}
fn marker(format: usize, phase: u32, p: usize) -> u16 {
    let x = p % 64;
    let y = p / 64;
    if format == 1 {
        if (x + y + phase as usize) & 1 == 0 {
            0
        } else {
            0x3c00
        }
    } else {
        (0x4400 + (((3 * x + 5 * y + 7 * phase as usize) & 7) << 6)) as u16
            | ((((x + y + phase as usize) & 1) as u16) << 15)
    }
}
fn seed(id: usize) -> Vec<u8> {
    const MIX: [u16; 16] = [
        0x3c00, 0xbc00, 0x3800, 0xb800, 1, 0x8001, 0x03ff, 0x83ff, 0x0400, 0x8400, 0x3bff, 0xbbff,
        0x3c01, 0xbc01, 0, 0x8000,
    ];
    const TINY: [u16; 16] = [
        1, 0x8001, 2, 0x8002, 0x01ff, 0x81ff, 0x0200, 0x8200, 0x03fe, 0x83fe, 0x03ff, 0x83ff,
        0x0400, 0x8400, 0, 0x8000,
    ];
    const NEAR: [u16; 16] = [
        0x3bfe, 0x3bff, 0x3c00, 0x3c01, 0x3c02, 0xbbfe, 0xbbff, 0xbc00, 0xbc01, 0xbc02, 0x37ff,
        0x3800, 0x3801, 0xb7ff, 0xb800, 0xb801,
    ];
    let table = match id {
        0 => MIX,
        1 => TINY,
        _ => NEAR,
    };
    let mut b = Vec::new();
    for p in 0..N {
        let x = p % 64;
        let y = p / 64;
        let i = (3 * x + 5 * y + 7 * (x / 2) + x * y) & 15;
        if id == 3 {
            b.extend_from_slice(&[255, 0, 128, if i & 1 == 0 { 0 } else { 255 }]);
        } else {
            for h in [
                MIX[(i + 3) & 15],
                MIX[(i + 7) & 15],
                MIX[(i + 11) & 15],
                table[i],
            ] {
                b.extend(h.to_le_bytes());
            }
        }
    }
    b
}
fn alpha(seed_id: usize) -> Vec<u16> {
    let b = seed(seed_id);
    (0..N)
        .map(|p| {
            if seed_id == 3 {
                if b[p * 4 + 3] == 0 {
                    0
                } else {
                    0x3c00
                }
            } else {
                half(&b, p * 8 + 6)
            }
        })
        .collect()
}
struct Prepared {
    inputs: Vec<u8>,
    snapshots: Vec<u8>,
    tsv: String,
    rounding: Vec<u8>,
    per_draw: Vec<Vec<Vec<u16>>>,
}
fn prepared() -> Prepared {
    let mut v = Prepared {
        inputs: (0..4).flat_map(seed).collect(),
        snapshots: Vec::new(),
        tsv: HEADER.into(),
        rounding: ROUND.into_iter().flat_map(u32::to_le_bytes).collect(),
        per_draw: Vec::new(),
    };
    let mut ids = BTreeMap::<Vec<u16>, usize>::new();
    for (id, c) in cases().iter().enumerate() {
        let mut state = alpha(c.seed);
        let mut ds = Vec::new();
        for (di, d) in c.draws.iter().enumerate() {
            let sid = if let Some(i) = ids.get(&state) {
                *i
            } else {
                let i = ids.len();
                ids.insert(state.clone(), i);
                v.snapshots
                    .extend(state.iter().flat_map(|h| h.to_le_bytes()));
                i
            };
            ds.push(state.clone());
            let values = [
                id,
                c.format,
                c.seed,
                usize::from(c.split),
                di,
                d.rect[0] as usize,
                d.rect[1] as usize,
                d.rect[2] as usize,
                d.rect[3] as usize,
                d.sc[0] as usize,
                d.sc[1] as usize,
                d.sc[2] as usize,
                d.sc[3] as usize,
                d.diag as usize,
                d.wind as usize,
                d.mode as usize,
                d.phase as usize,
                d.scale as usize,
                sid,
            ];
            v.tsv.push_str(&values.map(|n| n.to_string()).join("\t"));
            v.tsv.push('\n');
            if d.mode == 1 {
                for (p, h) in state.iter_mut().enumerate() {
                    if covered(*d, p) {
                        *h = marker(c.format, d.phase, p);
                    }
                }
            }
        }
        v.per_draw.push(ds);
    }
    v
}
fn prepare(root: &Path) -> Result<()> {
    let p = prepared();
    for (n, b) in [
        ("inputs.bin", p.inputs.as_slice()),
        ("snapshots.bin", &p.snapshots),
        ("rounding.bin", &p.rounding),
        ("cases.tsv", p.tsv.as_bytes()),
    ] {
        write_new(root, n, b)?;
    }
    Ok(())
}
fn plane(c: &Case) -> usize {
    2 * G + N * if c.format == 0 { 8 } else { 4 }
}
fn raw_size(c: &Case) -> usize {
    32 + 2 * plane(c) + 2 * G + count(c) * RECORD
}
fn cap(name: &str) -> usize {
    for (id, c) in cases().iter().enumerate() {
        if name == format!("case{id:02}.bin") {
            return raw_size(c);
        }
    }
    match name {
        "inputs.bin" => 3 * N * 8 + N * 4,
        "snapshots.bin" => prepared().snapshots.len(),
        "rounding.bin" => 128,
        "cases.tsv" => 16384,
        "native-cases.jsonl" => 32768,
        "environment.json" => 8192,
        "completion.json" | "failure.json" | "command-exit.json" | "negative.json"
        | "check.json" => 8192,
        "comparisons.tsv" => 32768,
        "source-pins.tsv" | "SHA256SUMS" => 16384,
        "binding.txt" => 256,
        _ => 0,
    }
}
fn bounds(root: &Path) -> Result<()> {
    need(
        io(fs::symlink_metadata(root))?.file_type().is_dir(),
        "root_type",
    )?;
    let mut total = 0;
    let mut count = 0;
    for e in io(fs::read_dir(root))? {
        let e = io(e)?;
        let m = io(fs::symlink_metadata(e.path()))?;
        let n = e.file_name().into_string().map_err(|_| "file_name")?;
        need(
            m.file_type().is_file() && cap(&n) > 0 && m.len() <= cap(&n) as u64,
            "artifact_allowlist_or_size",
        )?;
        total += m.len();
        count += 1;
    }
    need(total <= LIMIT && count <= 40, "artifact_budget")
}

// Only finite conversion controls use this RTNE routine. NEVER used to compute
// derivative expectations or helper operands. Subnormal/zero controls observe,
// rather than require, preservation under the requested fast compile profile.
fn round_half(bits: u32) -> u16 {
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 255) as i32 - 127;
    let mant = (bits & 0x7fffff) | 0x800000;
    if exp < -25 {
        return sign;
    }
    if exp > 15 {
        return sign | 0x7c00;
    }
    let shift = if exp < -14 { (-exp - 1) as u32 } else { 13 };
    let base = mant >> shift;
    let tail = mant & ((1 << shift) - 1);
    let mid = 1 << (shift - 1);
    let rounded = base + u32::from(tail > mid || (tail == mid && base & 1 != 0));
    if exp < -14 {
        sign | rounded as u16
    } else {
        sign | ((((exp + 14) as u32) << 10) + rounded) as u16
    }
}
fn widen(h: u16) -> f32 {
    let e = ((h >> 10) & 31) as i32;
    let m = (h & 1023) as f32;
    let v = if e == 0 {
        m / 16777216.0
    } else {
        (1.0 + m / 1024.0) * 2.0_f32.powi(e - 15)
    };
    if h & 0x8000 == 0 {
        v
    } else {
        -v
    }
}
fn hash(r: &[u32; 24]) -> u32 {
    r[..23]
        .iter()
        .fold(2166136261, |a, b| (a ^ b).wrapping_mul(16777619))
}
fn numbers(s: &str) -> Result<BTreeMap<String, u64>> {
    let t = s
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or("receipt_object")?;
    let mut m = BTreeMap::new();
    for item in t.split(',') {
        let (k, v) = item.split_once(':').ok_or("receipt_field")?;
        let k = k
            .trim()
            .strip_prefix('"')
            .and_then(|k| k.strip_suffix('"'))
            .ok_or("receipt_key")?;
        let v = v.trim();
        need(
            !k.is_empty()
                && k.bytes().all(|c| c.is_ascii_lowercase() || c == b'_')
                && !v.is_empty()
                && v.bytes().all(|c| c.is_ascii_digit())
                && (v == "0" || !v.starts_with('0')),
            "receipt_type",
        )?;
        let n = v.parse().map_err(|_| "receipt_number")?;
        need(m.insert(k.to_string(), n).is_none(), "receipt_duplicate")?;
    }
    Ok(m)
}
fn number_file(root: &Path, name: &str) -> Result<BTreeMap<String, u64>> {
    let b = plain_read(root, name, cap(name))?;
    numbers(std::str::from_utf8(&b).map_err(|_| "receipt_utf8")?)
}
fn map(v: &[(&str, usize)]) -> BTreeMap<String, u64> {
    v.iter().map(|(k, v)| (k.to_string(), *v as u64)).collect()
}
fn native_receipt(s: &str, id: usize, c: &Case) -> Result<()> {
    let commands = if c.split { c.draws.len() } else { 1 };
    let m = map(&[
        ("id", id),
        ("format", c.format),
        ("seed", c.seed),
        ("draws", c.draws.len()),
        ("commands", commands),
        ("completed", commands),
        ("errors", 0),
        ("width", 64),
        ("height", 64),
        ("pitch", if c.format == 0 { 512 } else { 256 }),
        ("samples", 1),
        ("primary_format_matches", 1),
        ("shared", 1),
        ("primary_load", 1),
        ("primary_store", 1),
        ("secondary_rg_half", 1),
        ("secondary_clear", 1),
        ("secondary_discard", 1),
        ("primary_write_all", 1),
        ("secondary_write_rg", 1),
        ("blend_disabled", 1),
        ("cull_none", 1),
        ("reciprocal_w_one", 1),
        ("trace_guards", 1),
        ("records", count(c)),
        ("bytes", raw_size(c)),
    ]);
    need(numbers(s)? == m, "native_receipt")
}
fn environment(root: &Path) -> Result<()> {
    let mut m = number_file(root, "environment.json")?;
    for key in [
        "default_math_mode_raw",
        "default_math_functions_raw",
        "effective_math_mode_raw",
        "effective_math_functions_raw",
    ] {
        need(
            m.remove(key).is_some_and(|v| v <= 16),
            "compile_enum_receipt",
        )?;
    }
    for key in [
        "default_fast_math",
        "unified_memory",
        "apple_seven",
        "mac_two",
    ] {
        need(m.remove(key).is_some_and(|v| v <= 1), "environment_boolean")?;
    }
    need(
        m == map(&[
            ("schema", 1),
            ("os_major", 26),
            ("arm_sixty_four", 1),
            ("language_two_four", 1),
            ("effective_fast_math", 1),
            ("effective_mode_fast", 1),
            ("effective_functions_fast", 1),
            ("denorm_override_requested", 0),
            ("ir_math_equivalence_claimed", 0),
            ("scalar_half_primary", 1),
            ("helper_values_observed", 0),
            ("record_bytes", RECORD),
            ("parameter_bytes", 80),
        ]),
        "effective_compile_receipt",
    )
}

#[derive(Default, Clone)]
struct Stats(BTreeMap<&'static str, u64>);
impl Stats {
    fn add(&mut self, k: &'static str, n: u64) {
        *self.0.entry(k).or_default() += n;
    }
    fn get(&self, k: &str) -> u64 {
        *self.0.get(k).unwrap_or(&0)
    }
    fn merge(&mut self, o: &Self) {
        for (k, v) in &o.0 {
            self.add(k, *v);
        }
    }
}
struct Capture {
    rows: Vec<Vec<Option<[u32; 24]>>>,
    final_bytes: Vec<u8>,
    stats: Stats,
}
fn rgba_close(observed: u16, expected: u16) -> bool {
    observed == expected
        || (subnormal(expected) && observed & 0x7fff == 0)
        || (expected & 0x7fff == 0 && observed & 0x7fff == 0)
        || (expected & 0x7fff > 0x7c00 && observed & 0x7fff > 0x7c00)
}
fn validate_case(raw: &[u8], id: usize, c: &Case, snapshots: &[Vec<u16>]) -> Result<Capture> {
    need(raw.len() == raw_size(c), "raw_size")?;
    let expected = [
        MAGIC,
        1,
        id as u32,
        c.format as u32,
        c.draws.len() as u32,
        count(c) as u32,
        plane(c) as u32,
        RECORD as u32,
    ];
    need(
        (0..8).all(|i| word(raw, i * 4) == expected[i]),
        "raw_header",
    )?;
    let bpp = if c.format == 0 { 8 } else { 4 };
    let plen = plane(c);
    for start in [32, 32 + plen] {
        need(
            raw[start..start + G]
                .iter()
                .chain(raw[start + G + N * bpp..start + plen].iter())
                .all(|x| *x == 0xa5),
            "plane_guard",
        )?;
    }
    need(
        raw[32 + G..32 + G + N * bpp] == seed(c.seed),
        "native_upload_roundtrip",
    )?;
    let final_bytes = raw[32 + plen + G..32 + plen + G + N * bpp].to_vec();
    let ts = 32 + plen * 2;
    need(
        raw[ts..ts + 2 * G].iter().all(|b| *b == 0xa5),
        "trace_guard",
    )?;
    let mut rows = vec![vec![None; N]; c.draws.len()];
    let mut stats = Stats::default();
    let mut prev = None;
    for r in raw[ts + 2 * G..].chunks_exact(RECORD) {
        let r: [u32; 24] = std::array::from_fn(|i| word(r, i * 4));
        let pixel = (r[0] & 4095) as usize;
        let di = ((r[0] >> 12) & 15) as usize;
        let primitive = r[1] as usize;
        need(
            di < c.draws.len()
                && primitive < 2
                && r[0] == 0xa7000000 | (id as u32) << 16 | (di as u32) << 12 | pixel as u32,
            "trace_stamp",
        )?;
        need(r[23] == hash(&r), "trace_hash")?;
        let key = (di, primitive, pixel);
        need(prev.is_none_or(|k| k < key), "trace_order")?;
        prev = Some(key);
        let d = c.draws[di];
        need(
            covered(d, pixel) && rows[di][pixel].is_none(),
            "trace_coverage_or_duplicate",
        )?;
        let side = side(d, pixel);
        need(
            side == 0 || primitive == usize::from(side > 0),
            "strict_primitive_ownership",
        )?;
        need(
            r[17] == (pixel as f32 % 64.0 + 0.5).to_bits()
                && r[18] == ((pixel / 64) as f32 + 0.5).to_bits(),
            "pixel_center",
        )?;
        need(
            r[22] == 0 && high(r[19]) == d.scale,
            "trace_reserved_or_scale",
        )?;
        let a = high(r[3]);
        let snapshot = low(r[8]);
        let expected = snapshots[di][pixel];
        need(finite(a) && finite(snapshot), "finite_covered_input")?;
        if normal(expected) || c.format == 1 {
            need(
                a == expected && snapshot == expected,
                "normal_snapshot_anchor",
            )?;
        }
        stats.add(
            "covered_snapshot_operand_differences",
            u64::from(a != snapshot),
        );
        stats.add(
            "snapshot_load_bit_differences",
            u64::from(snapshot != expected),
        );
        stats.add("half_subnormal_inputs", u64::from(subnormal(expected)));
        stats.add("half_subnormal_fetches", u64::from(subnormal(a)));
        stats.add("negative_half_inputs", u64::from(expected & 0x8000 != 0));
        need(
            high(r[6]) == 0 && low(r[7]) == 0x3c00 && low(r[6]) == high(r[11]),
            "half_output_stages",
        )?;
        let wanted_a = if d.mode == 0 {
            a
        } else {
            marker(c.format, d.phase, pixel)
        };
        need(high(r[7]) == wanted_a, "mutation_anchor")?;
        let ri = (pixel % 64 + 3 * (pixel / 64)) & 31;
        need(r[12] == ROUND[ri] && r[13] <= 65535, "round_input")?;
        let re = round_half(r[12]);
        if normal(re) {
            need(r[13] == re as u32, "normal_rounding_control")?;
            stats.add("normal_rounding_controls", 1);
        } else {
            stats.add("subnormal_zero_rounding_observations", 1);
        }
        need(
            r[20] == ROUND[30 + ((pixel % 64 + pixel / 64) & 1)],
            "binary32_denorm_input",
        )?;
        stats.add(
            if r[21] & 0x7fffffff == 0 {
                "binary32_product_zero"
            } else if r[21] == (r[20] & 0x80000000) | 0x00400000 {
                "binary32_product_subnormal"
            } else {
                "binary32_product_other"
            },
            1,
        );
        let fw = low(r[5]);
        let imm = high(r[8]);
        stats.add("records", 1);
        stats.add("direct_nonfinite_width", u64::from(!finite(fw)));
        stats.add("immutable_width_differences", u64::from(fw != imm));
        stats.add(
            "scissor_masked_width_differences",
            u64::from(fw != high(r[10])),
        );
        stats.add(
            "rectangle_masked_width_differences",
            u64::from(fw != low(r[11])),
        );
        stats.add(
            "direct_vs_separate_width_differences",
            u64::from(fw != high(r[5])),
        );
        stats.add(
            "half_vs_float_width_differences",
            u64::from(widen(fw).to_bits() != r[16]),
        );
        if strict_quad(d, pixel) && d.mode == 1 && finite(fw) && fw == imm {
            stats.add(
                "forward_refresh_witnesses",
                u64::from(primitive == 1 && imm != low(r[9])),
            );
            stats.add(
                "reverse_refresh_witnesses",
                u64::from(primitive == 0 && imm != high(r[9])),
            );
            stats.add("postdraw_live_witnesses", u64::from(imm != low(r[10])));
        }
        if finite(fw) && fw == imm {
            stats.add(
                "scissor_visibility_witnesses",
                u64::from(imm != high(r[10])),
            );
            stats.add(
                "rectangle_visibility_witnesses",
                u64::from(imm != low(r[11])),
            );
        }
        rows[di][pixel] = Some(r);
    }
    let before = seed(c.seed);
    for pixel in 0..N {
        for (di, d) in c.draws.iter().enumerate() {
            need(
                rows[di][pixel].is_some() == covered(*d, pixel),
                "missing_covered_record",
            )?;
        }
        let last = rows.iter().rev().find_map(|rs| rs[pixel]);
        if let Some(r) = last {
            if c.format == 0 {
                for (ch, e) in [low(r[6]), high(r[6]), low(r[7]), high(r[7])]
                    .into_iter()
                    .enumerate()
                {
                    let actual = half(&final_bytes, pixel * 8 + 2 * ch);
                    need(rgba_close(actual, e), "normal_final_output")?;
                    stats.add("subnormal_zero_store_differences", u64::from(actual != e));
                }
            } else {
                need(
                    final_bytes[pixel * 4] == 255
                        && final_bytes[pixel * 4 + 1] == 0
                        && final_bytes[pixel * 4 + 3] == if high(r[7]) == 0 { 0 } else { 255 },
                    "bgra_exact_channels",
                )?;
            }
        } else {
            need(
                final_bytes[pixel * bpp..(pixel + 1) * bpp]
                    == before[pixel * bpp..(pixel + 1) * bpp],
                "outside_visibility_store",
            )?;
        }
    }
    Ok(Capture {
        rows,
        final_bytes,
        stats,
    })
}

// SHA-256 is solely artifact/source integrity, not an oracle. Standard test vectors below.
fn sha256(b: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut bytes = b.to_vec();
    bytes.push(128);
    while bytes.len() % 64 != 56 {
        bytes.push(0);
    }
    bytes.extend((b.len() as u64 * 8).to_be_bytes());
    for chunk in bytes.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let a = w[i - 15];
            let z = w[i - 2];
            w[i] = w[i - 16]
                .wrapping_add(a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3))
                .wrapping_add(w[i - 7])
                .wrapping_add(z.rotate_right(17) ^ z.rotate_right(19) ^ (z >> 10));
        }
        let [mut a, mut c, mut d, mut e, mut f, mut g, mut j, mut k] = h;
        for i in 0..64 {
            let t = k
                .wrapping_add(f.rotate_right(6) ^ f.rotate_right(11) ^ f.rotate_right(25))
                .wrapping_add((f & g) ^ (!f & j))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let u = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & c) ^ (a & d) ^ (c & d));
            k = j;
            j = g;
            g = f;
            f = e.wrapping_add(t);
            e = d;
            d = c;
            c = a;
            a = t.wrapping_add(u);
        }
        for (i, v) in [a, c, d, e, f, g, j, k].into_iter().enumerate() {
            h[i] = h[i].wrapping_add(v);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let r = io(Command::new("git").current_dir(root).args(args).output())?;
    need(r.status.success(), "git_binding")?;
    Ok(String::from_utf8(r.stdout)
        .map_err(|_| "git_utf8")?
        .trim()
        .into())
}
fn head(h: &str) -> Result<()> {
    need(
        h.len() == 40
            && h.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "expected_head",
    )
}
fn source_pins(source: &Path, expected: &str) -> Result<String> {
    head(expected)?;
    need(
        git(source, &["rev-parse", "HEAD"])? == expected,
        "head_binding",
    )?;
    let mut s = String::from("path\tbytes\tsha256\tgit_blob\n");
    for path in SOURCE {
        let b = plain_read(source, path, 131072)?;
        let blob = git(source, &["rev-parse", &format!("{expected}:{path}")])?;
        need(
            blob == git(source, &["hash-object", "--", path])?,
            "source_blob_binding",
        )?;
        s.push_str(&format!("{path}\t{}\t{}\t{blob}\n", b.len(), sha256(&b)));
    }
    Ok(s)
}
fn binding(root: &Path, expected: &str, source: &Path) -> Result<()> {
    need(
        plain_read(root, "binding.txt", 256)?
            == format!(
                "reviewed_head={expected}\ndispatch_head={expected}\nchecked_head={expected}\n"
            )
            .as_bytes(),
        "binding_receipt",
    )?;
    need(
        plain_read(root, "source-pins.tsv", 16384)? == source_pins(source, expected)?.as_bytes(),
        "source_pins",
    )
}
fn seal_text(root: &Path) -> Result<String> {
    bounds(root)?;
    let mut files = BTreeMap::new();
    for e in io(fs::read_dir(root))? {
        let e = io(e)?;
        let n = e.file_name().into_string().map_err(|_| "file_name")?;
        if n != "SHA256SUMS" {
            files.insert(n, io(fs::read(e.path()))?);
        }
    }
    Ok(files
        .into_iter()
        .map(|(n, b)| format!("{}  {n}\n", sha256(&b)))
        .collect())
}

struct Checked {
    json: String,
    comparisons: String,
}
fn acquire(root: &Path, expected: &str, source: &Path) -> Result<Checked> {
    bounds(root)?;
    binding(root, expected, source)?;
    environment(root)?;
    need(
        number_file(root, "completion.json")?
            == map(&[
                ("schema", 1),
                ("cases", 24),
                ("draws", 30),
                ("commands", 26),
                ("status", 1),
            ]),
        "completion",
    )?;
    need(
        !root.join("failure.json").exists(),
        "native_failure_present",
    )?;
    let p = prepared();
    for (n, b) in [
        ("inputs.bin", p.inputs.as_slice()),
        ("snapshots.bin", &p.snapshots),
        ("rounding.bin", &p.rounding),
        ("cases.tsv", p.tsv.as_bytes()),
    ] {
        need(
            plain_read(root, n, cap(n))? == b,
            "synthetic_input_identity",
        )?;
    }
    let b = plain_read(root, "native-cases.jsonl", 32768)?;
    let receipt = std::str::from_utf8(&b).map_err(|_| "receipt_utf8")?;
    let lines: Vec<_> = receipt.lines().collect();
    need(lines.len() == 24, "case_receipt_count")?;
    let mut captures = Vec::new();
    let mut total = Stats::default();
    let mut tsv = String::from("case\tmetric\tcount\n");
    for (id, c) in cases().iter().enumerate() {
        native_receipt(lines[id], id, c)?;
        let raw = plain_read(root, &format!("case{id:02}.bin"), raw_size(c))?;
        let capture = validate_case(&raw, id, c, &p.per_draw[id])?;
        for (k, v) in &capture.stats.0 {
            tsv.push_str(&format!("{id}\t{k}\t{v}\n"));
        }
        total.merge(&capture.stats);
        captures.push(capture);
    }
    for (a, b) in [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (8, 9),
        (10, 11),
        (12, 13),
        (14, 15),
        (16, 17),
        (22, 23),
    ] {
        for pixel in 0..N {
            if let (Some(x), Some(y)) = (captures[a].rows[0][pixel], captures[b].rows[0][pixel]) {
                total.add("paired_covered_rows", 1);
                total.add(
                    "paired_direct_stage_differences",
                    u64::from(
                        high(x[3]) != high(y[3])
                            || [4, 5, 14, 15, 16, 19].into_iter().any(|i| x[i] != y[i]),
                    ),
                );
            }
        }
    }
    for di in 0..3 {
        for pixel in 0..N {
            if let (Some(x), Some(y)) = (captures[18].rows[di][pixel], captures[20].rows[di][pixel])
            {
                total.add(
                    "same_vs_separate_pass_stage_differences",
                    u64::from(x[1..23] != y[1..23]),
                );
            }
        }
    }
    total.add(
        "same_vs_separate_pass_final_differences",
        u64::from(captures[18].final_bytes != captures[20].final_bytes),
    );
    // This result is a restricted, native-vs-native diagnostic, NOT permission
    // to widen a production backend or a guessed native helper implementation.
    let rgba_witnesses = [3, 5, 7, 9].into_iter().all(|i| {
        [
            "forward_refresh_witnesses",
            "reverse_refresh_witnesses",
            "postdraw_live_witnesses",
            "rectangle_visibility_witnesses",
        ]
        .iter()
        .all(|k| captures[i].stats.get(k) > 0)
    }) && [15, 17]
        .into_iter()
        .all(|i| captures[i].stats.get("scissor_visibility_witnesses") > 0);
    let diagnostic = rgba_witnesses
        && total.get("immutable_width_differences") == 0
        && total.get("covered_snapshot_operand_differences") == 0
        && total.get("paired_direct_stage_differences") == 0
        && total.get("direct_nonfinite_width") == 0
        && total.get("same_vs_separate_pass_stage_differences") == 0
        && total.get("same_vs_separate_pass_final_differences") == 0
        && total.get("binary32_product_other") == 0
        && [
            "forward_refresh_witnesses",
            "reverse_refresh_witnesses",
            "postdraw_live_witnesses",
            "scissor_visibility_witnesses",
            "rectangle_visibility_witnesses",
        ]
        .iter()
        .all(|k| total.get(k) > 0);
    let mut json=format!("{{\"schema\":1,\"capture_integrity_passed\":true,\"native_observations_present\":true,\"cases\":24,\"restricted_immutable_diagnostic_supported\":{diagnostic},\"production_contract_qualified\":false,\"native_helper_values_observed\":false,\"ir_math_mode_equivalence_claimed\":false,\"fractional_geometry_qualified\":false,\"counts\":{{");
    json.push_str(
        &total
            .0
            .iter()
            .map(|(k, v)| format!("\"{k}\":{v}"))
            .collect::<Vec<_>>()
            .join(","),
    );
    json.push_str("}}\n");
    Ok(Checked {
        json,
        comparisons: tsv,
    })
}
const NEGATIVE: &str = "{\"healthy_acquisition_rechecked\":true,\"truncated_rejected\":true,\"header_rejected\":true,\"guard_rejected\":true,\"payload_hash_rejected\":true,\"wrong_normal_anchor_rejected\":true,\"controls\":5}\n";
fn negative(root: &Path, expected: &str, source: &Path) -> Result<()> {
    let healthy = acquire(root, expected, source)?;
    need(
        plain_read(root, "check.json", 8192)? == healthy.json.as_bytes(),
        "healthy_check_missing",
    )?;
    let c = &cases()[0];
    let p = prepared();
    let original = plain_read(root, "case00.bin", raw_size(c))?;
    let at = 32 + plane(c) * 2 + G * 2;
    for (mutation, reason) in [
        (0, "raw_size"),
        (1, "raw_header"),
        (2, "plane_guard"),
        (3, "trace_hash"),
        (4, "normal_snapshot_anchor"),
    ] {
        let mut b = original.clone();
        match mutation {
            0 => {
                b.pop();
            }
            1 => {
                b[0] ^= 1;
            }
            2 => {
                b[32] ^= 1;
            }
            3 => {
                b[at + 16] ^= 1;
            }
            _ => {
                b[at + 3 * 4 + 2] ^= 1;
                let r = std::array::from_fn(|i| word(&b, at + i * 4));
                b[at + 23 * 4..at + 24 * 4].copy_from_slice(&hash(&r).to_le_bytes());
            }
        }
        need(
            matches!(validate_case(&b,0,c,&p.per_draw[0]),Err(e) if e==reason),
            "negative_not_rejected_exactly",
        )?;
    }
    write_new(root, "negative.json", NEGATIVE.as_bytes())
}
fn run() -> Result<i32> {
    let a: Vec<_> = env::args().collect();
    need(a.len() >= 3, "usage_command_results")?;
    let root = Path::new(&a[2]);
    match a[1].as_str() {
        "prepare" => {
            need(a.len() == 3, "usage")?;
            prepare(root)?;
        }
        "bounds" => {
            need(a.len() == 3, "usage")?;
            bounds(root)?;
        }
        "seal" => {
            need(a.len() == 3, "usage")?;
            let text = seal_text(root)?;
            write_new(root, "SHA256SUMS", text.as_bytes())?;
            bounds(root)?;
        }
        "bind" | "check" | "negative" | "verify" => {
            need(a.len() == 5, "usage_expected_head_source_root")?;
            let expected = &a[3];
            let source = Path::new(&a[4]);
            if a[1] == "bind" {
                head(expected)?;
                need(
                    env::var("EXPECTED_HEAD").ok().as_deref() == Some(expected)
                        && env::var("GITHUB_SHA").ok().as_deref() == Some(expected),
                    "dispatch_binding",
                )?;
                let pins = source_pins(source, expected)?;
                write_new(root, "source-pins.tsv", pins.as_bytes())?;
                write_new(root,"binding.txt",format!("reviewed_head={expected}\ndispatch_head={expected}\nchecked_head={expected}\n").as_bytes())?;
            } else if a[1] == "negative" {
                negative(root, expected, source)?;
                return Ok(2);
            } else {
                let c = acquire(root, expected, source)?;
                if a[1] == "check" {
                    write_new(root, "check.json", c.json.as_bytes())?;
                    write_new(root, "comparisons.tsv", c.comparisons.as_bytes())?;
                } else {
                    need(
                        plain_read(root, "check.json", 8192)? == c.json.as_bytes()
                            && plain_read(root, "comparisons.tsv", 32768)?
                                == c.comparisons.as_bytes(),
                        "checked_receipts",
                    )?;
                    need(
                        plain_read(root, "negative.json", 8192)? == NEGATIVE.as_bytes(),
                        "negative_receipt",
                    )?;
                    need(
                        plain_read(root, "command-exit.json", 8192)?
                            == b"{\"exit\":0,\"phase\":\"complete\"}\n",
                        "command_completion",
                    )?;
                    need(
                        plain_read(root, "SHA256SUMS", 16384)? == seal_text(root)?.as_bytes(),
                        "seal_mismatch",
                    )?;
                    println!("SEALED_NATIVE_ACQUISITION_VALID_NOT_PRODUCTION_QUALIFICATION");
                }
            }
        }
        _ => return Err("unknown_command".into()),
    }
    Ok(0)
}
fn main() {
    match run() {
        Ok(n) => process::exit(n),
        Err(e) => {
            eprintln!("NOT_PROOF {e}");
            process::exit(3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cohort_bounds_and_coverage() {
        let cs = cases();
        let p = prepared();
        assert_eq!(cs.len(), 24);
        assert_eq!(cs.iter().map(|c| c.draws.len()).sum::<usize>(), 30);
        assert_eq!(
            cs.iter()
                .map(|c| if c.split { c.draws.len() } else { 1 })
                .sum::<usize>(),
            26
        );
        let total = cs.iter().map(raw_size).sum::<usize>()
            + p.inputs.len()
            + p.snapshots.len()
            + p.tsv.len()
            + 128
            + 128 * 1024;
        assert!(total <= LIMIT as usize, "{total}");
        assert_eq!(cs.iter().map(count).sum::<usize>(), 17454);
        for c in &cs {
            for d in &c.draws {
                assert!(d.rect[0] + d.rect[2] <= 64 && d.rect[1] + d.rect[3] <= 64);
                assert!(d.sc[0] + d.sc[2] <= 64 && d.sc[1] + d.sc[3] <= 64);
            }
        }
        assert!(cs.iter().any(|c| c.draws.iter().any(|d| d.sc[2] & 1 == 1)));
        for diag in 0..2 {
            for wind in 0..2 {
                assert!(cs
                    .iter()
                    .any(|c| c.draws.iter().any(|d| d.diag == diag && d.wind == wind)));
            }
        }
    }
    #[test]
    fn finite_signed_subnormal_seeds() {
        for id in 0..3 {
            let a = alpha(id);
            assert!(a.iter().all(|h| finite(*h)));
            assert!(a.iter().any(|h| *h & 0x8000 != 0));
        }
        assert!(alpha(0).iter().any(|h| subnormal(*h)));
        assert!(alpha(1).iter().any(|h| subnormal(*h) && *h & 0x8000 != 0));
    }
    #[test]
    fn snapshots_refresh_only_between_draws() {
        let p = prepared();
        let cs = cases();
        assert_eq!(p.per_draw[18], p.per_draw[20]);
        assert_ne!(p.per_draw[18][0], p.per_draw[18][1]);
        assert_ne!(p.per_draw[18][1], p.per_draw[18][2]);
        for pixel in 0..N {
            assert_eq!(
                p.per_draw[18][1][pixel],
                if covered(cs[18].draws[0], pixel) {
                    marker(0, 0, pixel)
                } else {
                    p.per_draw[18][0][pixel]
                }
            );
        }
    }
    #[test]
    fn normal_conversion_ties_and_signs() {
        for (f, h) in [
            (0x3f800000, 0x3c00),
            (0x3f801000, 0x3c00),
            (0x3f801001, 0x3c01),
            (0x3f803000, 0x3c02),
            (0xbf801000, 0xbc00),
            (0xbf803000, 0xbc02),
            (0x33800000, 1),
            (0x33000000, 0),
            (0x33000001, 1),
            (0x38800000, 0x0400),
        ] {
            assert_eq!(round_half(f), h);
        }
        for h in 0u16..0x7c00 {
            assert_eq!(round_half(widen(h).to_bits()), h);
            assert_eq!(round_half(widen(h | 0x8000).to_bits()), h | 0x8000);
        }
    }
    #[test]
    fn sha_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
    #[test]
    fn receipts_reject_missing_duplicate_extra_type() {
        for s in [
            "{}",
            "{\"x\":1,\"x\":2}",
            "{\"x\":true}",
            "{\"x\":null}",
            "{\"x\":-1}",
            "{\"x\":01}",
            "{\"x\":[]}",
        ] {
            assert!(numbers(s).is_err());
        }
        let c = &cases()[0];
        assert!(native_receipt("{\"id\":0}", 0, c).is_err());
    }
    #[test]
    fn wrong_head_and_unknown_allowlist_rejected() {
        for s in [
            "",
            "main",
            "../other",
            "012345678901234567890123456789012345678g",
        ] {
            assert!(head(s).is_err());
        }
        for s in [
            "README.md",
            "helper-values.bin",
            "../inputs.bin",
            "device.json",
            "raw.air",
            "case24.bin",
            "log.txt",
        ] {
            assert_eq!(cap(s), 0);
        }
    }
    // In-memory protocol fixtures ONLY. Their arithmetic stages are arbitrary
    // placeholders, never saved, sealed, or presented as native observations.
    fn fake(id: usize) -> Vec<u8> {
        let cs = cases();
        let c = &cs[id];
        let p = prepared();
        let plen = plane(c);
        let bpp = if c.format == 0 { 8 } else { 4 };
        let mut b = vec![0xa5; 32 + plen * 2 + G * 2];
        for (i, v) in [
            MAGIC,
            1,
            id as u32,
            c.format as u32,
            c.draws.len() as u32,
            count(c) as u32,
            plen as u32,
            RECORD as u32,
        ]
        .into_iter()
        .enumerate()
        {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for start in [32 + G, 32 + plen + G] {
            b[start..start + N * bpp].copy_from_slice(&seed(c.seed));
        }
        for (di, d) in c.draws.iter().enumerate() {
            for prim in 0..2 {
                for pixel in 0..N {
                    if !covered(*d, pixel) || usize::from(side(*d, pixel) > 0) != prim {
                        continue;
                    }
                    let a = p.per_draw[id][di][pixel];
                    let oa = if d.mode == 0 {
                        a
                    } else {
                        marker(c.format, d.phase, pixel)
                    };
                    let mut r = [0u32; 24];
                    r[0] = 0xa7000000 | (id as u32) << 16 | (di as u32) << 12 | pixel as u32;
                    r[1] = prim as u32;
                    r[3] = (a as u32) << 16;
                    r[7] = 0x3c00 | (oa as u32) << 16;
                    r[8] = a as u32;
                    r[19] = (d.scale as u32) << 16;
                    r[12] = ROUND[(pixel % 64 + 3 * (pixel / 64)) & 31];
                    r[13] = round_half(r[12]) as u32;
                    r[17] = (pixel as f32 % 64.0 + 0.5).to_bits();
                    r[18] = ((pixel / 64) as f32 + 0.5).to_bits();
                    r[20] = ROUND[30 + ((pixel % 64 + pixel / 64) & 1)];
                    r[23] = hash(&r);
                    let at = 32 + plen + G + pixel * bpp;
                    if c.format == 0 {
                        for (ch, h) in [0, 0, 0x3c00, oa].into_iter().enumerate() {
                            b[at + ch * 2..at + ch * 2 + 2].copy_from_slice(&h.to_le_bytes());
                        }
                    } else {
                        b[at..at + 4].copy_from_slice(&[255, 0, 0, if oa == 0 { 0 } else { 255 }]);
                    }
                    b.extend(r.into_iter().flat_map(u32::to_le_bytes));
                }
            }
        }
        b
    }
    #[test]
    fn protocol_only_fixtures_validate_all_cases() {
        let p = prepared();
        for (id, c) in cases().iter().enumerate() {
            assert!(
                validate_case(&fake(id), id, c, &p.per_draw[id]).is_ok(),
                "{id}"
            );
        }
    }
    #[test]
    fn all_trace_words_load_bearing_without_repaired_hash() {
        let c = &cases()[0];
        let p = prepared();
        let original = fake(0);
        let at = 32 + 2 * plane(c) + 2 * G;
        for i in 0..24 {
            let mut b = original.clone();
            b[at + 4 * i] ^= 1;
            assert!(validate_case(&b, 0, c, &p.per_draw[0]).is_err(), "word {i}");
        }
    }
    #[test]
    fn guards_headers_truncation_poison_fail_closed() {
        let c = &cases()[0];
        let p = prepared();
        let original = fake(0);
        for at in (0..8).map(|i| i * 4).chain([
            32,
            32 + G - 1,
            32 + plane(c) - 1,
            32 + plane(c),
            32 + 2 * plane(c),
            32 + 2 * plane(c) + 2 * G - 1,
        ]) {
            let mut b = original.clone();
            b[at] ^= 1;
            assert!(validate_case(&b, 0, c, &p.per_draw[0]).is_err());
        }
        assert!(validate_case(&original[..original.len() - 1], 0, c, &p.per_draw[0]).is_err());
        assert!(validate_case(&vec![0xa5; original.len()], 0, c, &p.per_draw[0]).is_err());
    }
    #[test]
    fn repaired_hash_wrong_anchor_and_coverage_rejected() {
        let c = &cases()[0];
        let p = prepared();
        let original = fake(0);
        let at = 32 + 2 * plane(c) + 2 * G;
        for off in [3 * 4 + 2, 13 * 4, 17 * 4, 19 * 4 + 2, 20 * 4, 22 * 4] {
            let mut b = original.clone();
            b[at + off] ^= 1;
            let r = std::array::from_fn(|i| word(&b, at + i * 4));
            b[at + 92..at + 96].copy_from_slice(&hash(&r).to_le_bytes());
            assert!(validate_case(&b, 0, c, &p.per_draw[0]).is_err());
        }
        let mut b = original.clone();
        b[at + RECORD..at + 2 * RECORD].copy_from_slice(&original[at..at + RECORD]);
        assert!(validate_case(&b, 0, c, &p.per_draw[0]).is_err());
    }
    #[test]
    fn native_receipt_every_field_is_load_bearing() {
        let c = &cases()[0];
        let fields = map(&[
            ("id", 0),
            ("format", 0),
            ("seed", 2),
            ("draws", 1),
            ("commands", 1),
            ("completed", 1),
            ("errors", 0),
            ("width", 64),
            ("height", 64),
            ("pitch", 512),
            ("samples", 1),
            ("primary_format_matches", 1),
            ("shared", 1),
            ("primary_load", 1),
            ("primary_store", 1),
            ("secondary_rg_half", 1),
            ("secondary_clear", 1),
            ("secondary_discard", 1),
            ("primary_write_all", 1),
            ("secondary_write_rg", 1),
            ("blend_disabled", 1),
            ("cull_none", 1),
            ("reciprocal_w_one", 1),
            ("trace_guards", 1),
            ("records", 4096),
            ("bytes", raw_size(c)),
        ]);
        let serialize = |m: &BTreeMap<String, u64>| {
            format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!("\"{k}\":{v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        assert!(native_receipt(&serialize(&fields), 0, c).is_ok());
        for k in fields.keys() {
            let mut m = fields.clone();
            *m.get_mut(k).unwrap() ^= 1;
            assert!(native_receipt(&serialize(&m), 0, c).is_err());
            m = fields.clone();
            m.remove(k);
            assert!(native_receipt(&serialize(&m), 0, c).is_err());
        }
        let mut m = fields;
        m.insert("extra".into(), 0);
        assert!(native_receipt(&serialize(&m), 0, c).is_err());
    }
    #[test]
    fn independent_half_channels_and_store_visibility_checked() {
        for id in [0, 2, 22] {
            let cs = cases();
            let c = &cs[id];
            let p = prepared();
            let original = fake(id);
            let first = 32 + plane(c) + G;
            let px = (0..N).find(|p| covered(c.draws[0], *p)).unwrap();
            let bpp = if c.format == 0 { 8 } else { 4 };
            let channel = if c.format == 0 { 2 } else { 1 };
            let mut b = original.clone();
            b[first + px * bpp + channel] ^= 1;
            assert!(validate_case(&b, id, c, &p.per_draw[id]).is_err());
            if let Some(px) = (0..N).find(|p| !covered(c.draws[0], *p)) {
                let mut b = original.clone();
                b[first + px * bpp] ^= 1;
                assert!(validate_case(&b, id, c, &p.per_draw[id]).is_err());
            }
        }
    }
}
