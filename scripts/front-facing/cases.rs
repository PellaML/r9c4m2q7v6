// Synthetic source inputs and a validator of actual GPU bytes, never a CPU-facing oracle.
#![forbid(unsafe_code)]
use std::{env, fs, io::Write, path::Path};

const W: usize = 10;
const H: usize = 10;
const PITCH: usize = 256;
const GUARD: usize = 256;
const LEN: usize = GUARD * 2 + H * PITCH;
const CLEAR: [u8; 4] = [0x91, 0xa2, 0xb3, 0xc4];
const POS: [[u32; 4]; 3] = [
    [0xbf800000, 0xbf800000, 0x3f000000, 0x3f800000], // (-1,-1,.5,1)
    [0x40400000, 0xbf800000, 0x3f000000, 0x3f800000], // ( 3,-1,.5,1)
    [0xbf800000, 0x40400000, 0x3f000000, 0x3f800000], // (-1, 3,.5,1)
];
const MODES: [&str; 7] = [
    "default",
    "default",
    "clockwise",
    "clockwise",
    "counterClockwise",
    "counterClockwise",
    "default",
];

fn vertices() -> Vec<u8> {
    [0, 1, 2, 0, 2, 1]
        .into_iter()
        .flat_map(|v| POS[v])
        .flat_map(u32::to_le_bytes)
        .collect()
}
fn seed() -> Vec<u8> {
    let mut bytes = vec![0xcd; LEN];
    for y in 0..H {
        for x in 0..W {
            let p = GUARD + y * PITCH + x * 4;
            bytes[p..p + 4].copy_from_slice(&CLEAR);
        }
    }
    bytes
}
fn manifest() -> String {
    let bits = POS
        .iter()
        .map(|p| {
            format!(
                "[{}]",
                p.iter()
                    .map(|v| format!("\"0x{v:08x}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let cases = MODES.iter().enumerate().map(|(i, mode)| format!(
        "{{\"id\":{i},\"mode\":\"{mode}\",\"order\":{},\"vertexOffset\":{},\"tag\":{},\"skipRow\":{}}}",
        i % 2, (i % 2) * 48, 65 + i, if i == 6 { 4 } else { u32::MAX }))
        .collect::<Vec<_>>().join(",\n");
    format!(
        r#"{{
"schema":1,"width":10,"height":10,"pitch":256,"guardBytes":256,
"viewport":[1,1,8,8,0,1],"scissor":[1,1,8,8],"samples":1,
"topology":"one triangle per fresh encoder; cullMode=none; fill=fill",
"ndc_bits_abc":[{bits}],"orders":[[0,1,2],[0,2,1]],
"viewport_map":"x=1+4*(ndc.x+1); y=1+4*(1-ndc.y); z=ndc.z",
"orient2d_definition":"cross(v1-v0,v2-v0), before any vertex swap or normalization",
"orient2d_ndc":[16,-16],"orient2d_viewport_y_down":[-256,256],
"format":"RGBA8Uint","channels":"R=uint(front_facing); G=tag; B=floor(position.x); A=floor(position.y)",
"texture_seed_rgba":[145,162,179,196],"buffer_guard_byte":205,
"layout":"256-byte prefix; 10 rows of 256 bytes (40 texel bytes + 216 padding); 256-byte suffix",
"negative":"case 6 calls discard_fragment at absolute pixel row 4, not a host edit",
"expected_front_facing":null,"cases":[{cases}]
}}
"#
    )
}
fn save(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_file() || m.len() > limit as u64 {
        return Err("invalid file type/size".into());
    }
    fs::read(path).map_err(|e| e.to_string())
}
fn generate(dir: &Path) -> Result<(), String> {
    fs::create_dir(dir).map_err(|e| format!("fresh input directory required: {e}"))?;
    for (name, bytes) in [
        ("cases.json", manifest().into_bytes()),
        ("vertices.bin", vertices()),
        ("seed.bin", seed()),
    ] {
        save(&dir.join(name), &bytes)?;
    }
    Ok(())
}

// This checks coverage/markers/guards and READS R. It never predicts R from winding.
fn inspect(raw: &[u8], tag: u8, skip: Option<usize>) -> Result<u8, String> {
    if raw.len() != LEN {
        return Err("readback length".into());
    }
    if raw[..GUARD]
        .iter()
        .chain(raw[GUARD + H * PITCH..].iter())
        .any(|&v| v != 0xcd)
    {
        return Err("prefix/suffix guard".into());
    }
    let mut bit = None;
    for y in 0..H {
        let row = GUARD + y * PITCH;
        if raw[row + W * 4..row + PITCH].iter().any(|&v| v != 0xcd) {
            return Err("row padding guard".into());
        }
        for x in 0..W {
            let p = &raw[row + x * 4..row + x * 4 + 4];
            let covered = (1..9).contains(&x) && (1..9).contains(&y) && skip != Some(y);
            if !covered {
                if p != CLEAR {
                    return Err(format!("seed/texture guard at {x},{y}"));
                }
            } else {
                if p[1..] != [tag, x as u8, y as u8] {
                    return Err(format!("coverage/marker at {x},{y}"));
                }
                if p[0] > 1 || bit.is_some_and(|v| v != p[0]) {
                    return Err("non-boolean/nonuniform R".into());
                }
                bit = Some(p[0]);
            }
        }
    }
    bit.ok_or_else(|| "no covered pixel".into())
}
fn validate(dir: &Path) -> Result<(), String> {
    for (name, bytes) in [
        ("cases.json", manifest().into_bytes()),
        ("vertices.bin", vertices()),
        ("seed.bin", seed()),
    ] {
        if read(&dir.join(name), bytes.len())? != bytes {
            return Err(format!("changed input {name}"));
        }
    }
    let receipt = String::from_utf8(read(&dir.join("gpu-commands.tsv"), 16384)?)
        .map_err(|e| e.to_string())?;
    let rows: Vec<_> = receipt.lines().collect();
    if rows.len() != 8 || rows[0] != "id\tstatus\terror_present\twinding_call\tvertex_count\tvertex_offset\tgpu_start\tgpu_end" {
        return Err("seven real completed GPU commands required".into());
    }
    let mut bits = Vec::new();
    let mut pixels = String::from("id\tx\ty\tr\tg\tb\ta\n");
    for i in 0..7 {
        let f: Vec<_> = rows[i + 1].split('\t').collect();
        let call = if MODES[i] == "default" {
            "omitted"
        } else {
            MODES[i]
        };
        if f.len() != 8
            || f[0] != i.to_string()
            || f[1] != "4"
            || f[2] != "0"
            || f[3] != call
            || f[4] != "3"
            || f[5] != (64 + (i % 2) * 48).to_string()
        {
            return Err(format!("incomplete/incorrect GPU command {i}"));
        }
        let raw = read(&dir.join(format!("case-{i}.gpu.bin")), LEN)?;
        bits.push(inspect(
            &raw,
            65 + i as u8,
            if i == 6 { Some(4) } else { None },
        )?);
        if i == 6 && inspect(&raw, 71, None).is_ok() {
            return Err("skipped-row negative qualified as full coverage".into());
        }
        for y in 0..H {
            for x in 0..W {
                let p = &raw[GUARD + y * PITCH + x * 4..][..4];
                pixels += &format!("{i}\t{x}\t{y}\t{}\t{}\t{}\t{}\n", p[0], p[1], p[2], p[3]);
            }
        }
    }
    if bits[0] == bits[1]
        || bits[2] == bits[3]
        || bits[4] == bits[5]
        || bits[2] == bits[4]
        || bits[3] == bits[5]
        || bits[6] != bits[0]
    {
        return Err("orientation/state complement or negative-control consistency failed".into());
    }
    let default_matches = if bits[..2] == bits[2..4] {
        "clockwise"
    } else if bits[..2] == bits[4..6] {
        "counterClockwise"
    } else {
        return Err("default did not match either explicit state".into());
    };
    let mut observations = String::from("id\tstate\torder\tndc_orient2d\tviewport_orient2d\tobserved_front_facing\tcovered_pixels\tqualification\n");
    for i in 0..7 {
        let (order, ndc, viewport) = if i % 2 == 0 {
            ("abc", 16, -256)
        } else {
            ("acb", -16, 256)
        };
        observations += &format!(
            "{i}\t{}\t{order}\t{ndc}\t{viewport}\t{}\t{}\t{}\n",
            MODES[i],
            bits[i],
            if i == 6 { 56 } else { 64 },
            if i == 6 {
                "NEGATIVE_REJECTED_AS_FULL_COVERAGE"
            } else {
                "OBSERVED"
            }
        );
    }
    save(&dir.join("pixels.tsv"), pixels.as_bytes())?;
    save(&dir.join("observations.tsv"), observations.as_bytes())?;
    save(&dir.join("validation.json"), format!("{{\"status\":\"OBSERVED_VALIDATED\",\"normal_observations\":6,\"completed_gpu_commands\":7,\"identical_baseline_coverage\":true,\"all_guards_unchanged\":true,\"gpu_skipped_row_negative_rejected\":true,\"default_matches_observed\":\"{default_matches}\",\"cpu_facing_prediction\":false}}\n").as_bytes())?;
    print!("{observations}");
    Ok(())
}
fn main() {
    let a: Vec<_> = env::args().collect();
    let result = match a.as_slice() {
        [_, op, dir] if op == "generate" => generate(Path::new(dir)),
        [_, op, dir] if op == "validate" => validate(Path::new(dir)),
        _ => Err("usage: cases generate NEW_DIRECTORY | validate CAPTURE_DIRECTORY".into()),
    };
    if let Err(e) = result {
        eprintln!("NOT_QUALIFIED: {e}");
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_source_bits_and_opposed_orders() {
        let v = vertices();
        assert_eq!(v.len(), 96);
        assert_eq!(&v[0..16], &v[48..64]);
        assert_eq!(&v[16..32], &v[80..96]);
        assert_eq!(&v[32..48], &v[64..80]);
        assert_eq!(&v[..4], &0xbf800000u32.to_le_bytes());
    }
    #[test]
    fn geometric_area_before_swap_and_viewport_y_flip() {
        let p = POS.map(|v| {
            [
                f64::from(f32::from_bits(v[0])),
                f64::from(f32::from_bits(v[1])),
            ]
        });
        let area = |p: [[f64; 2]; 3]| {
            (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0])
        };
        assert_eq!(area(p), 16.0);
        assert_eq!(area([p[0], p[2], p[1]]), -16.0);
        assert_eq!(
            area(p.map(|v| [1.0 + 4.0 * (v[0] + 1.0), 1.0 + 4.0 * (1.0 - v[1])])),
            -256.0
        );
    }
    #[test]
    fn seed_cannot_qualify_as_observation() {
        assert!(inspect(&seed(), 65, None).is_err());
        assert!(inspect(&seed(), 71, Some(4)).is_err());
    }
    #[test]
    fn truncated_readback_rejected() {
        assert!(inspect(&seed()[..LEN - 1], 65, None).is_err());
    }
    #[test]
    fn guards_and_format_are_fixed() {
        let s = seed();
        assert_eq!(s.len(), 3072);
        assert!(s[..256].iter().all(|&b| b == 205));
        assert_eq!(&s[256..260], &CLEAR);
        assert!(s[296..512].iter().all(|&b| b == 205));
    }
    #[test]
    fn inputs_have_no_facing_answer() {
        assert!(manifest().contains("\"expected_front_facing\":null"));
        assert_eq!(MODES.iter().filter(|&&m| m == "default").count(), 3);
    }
}
