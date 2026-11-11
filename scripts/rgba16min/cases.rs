// Generic fixed-function MIN observations, derived from the public rg16max harness.
// Safe, dependency-free Rust. Models are hypotheses, never native evidence.
#![forbid(unsafe_code)]
use std::{collections::BTreeMap, env, fs, io::Write, path::Path, process::Command};

type Result<T> = std::result::Result<T, String>;
const WIDTH: usize = 3;
const PITCH: usize = 256;
const GUARD: usize = 256;
const TRACE_GUARD: usize = 64;
const TRACE_STRIDE: usize = 32;
const POISON: u8 = 0xcd;
const MAGIC: u32 = 0x6d69_6e00;
const SKIP: usize = 2;
const MASKS: [u32; 8] = [0, 8, 4, 2, 1, 5, 10, 15];
const LANES: [u32; 4] = [8, 4, 2, 1];
const LIMIT: u64 = 4 * 1024 * 1024;
const SOURCE: [&str; 5] = [
    ".github/workflows/rgba16min.yml",
    "scripts/rgba16min/cases.rs",
    "scripts/rgba16min/probe.metal",
    "scripts/rgba16min/probe.swift",
    "scripts/rgba16min/run.sh",
];
const RECEIPT_HEADER: &str = "tag\tname\tstatus\terror\tgpu_start\tgpu_end\tformat\twidth\theight\tmask\tblending\trgb_op\talpha_op\tsrc_rgb\tdst_rgb\tsrc_alpha\tdst_alpha\treturn_bits\tpipeline_format\tsamples\talpha_to_coverage\talpha_to_one\trasterization\tload_action\tstore_action\tdraws\tvertices\tpso_created";
const ENUMS: &str = "name\tvalue\nrgba16Float\t115\nmin\t3\none\t1\nred\t8\ngreen\t4\nblue\t2\nalpha\t1\nall\t15\ncompleted\t4\nload\t1\nstore\t1\n";
const COMPLETE: &str = "schema=1\nstatus=0\ncommands=22\ncaptures=21\n";
const COMPILER: &str = "property\tvalue\nfastMathEnabled\t0\nlanguageVersion\t131076\nbinary32DenormControl\tnot-explicitly-set\nprofileScope\tthis-profile-only\n";
const EXIT_OK: &str = "{\"exit\":0,\"phase\":\"complete\"}\n";
const NEGATIVE: &str = "{\"schema\":1,\"wrong_expected_rejected\":true,\"native_bytes_unchanged\":true,\"capture\":\"f32-min-15\",\"row\":0,\"lane\":0,\"exit\":2}\n";

#[derive(Clone, Debug)]
struct Row {
    kind: &'static str,
    name: String,
    float: [u32; 4],
    half: [u16; 4],
    destination: [u16; 4],
}
#[derive(Clone, Debug)]
struct Spec {
    name: String,
    precision: u32,
    mode: &'static str,
    mask: u32,
    tag: u32,
    negative: bool,
}
fn need(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn hfloat(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = (h >> 10) & 31;
    let mant = u32::from(h & 1023);
    let bits = match exp {
        0 => ((mant as f32) * f32::from_bits(0x3380_0000)).to_bits(),
        31 => 0x7f80_0000 | (mant << 13),
        _ => ((u32::from(exp) + 112) << 23) | (mant << 13),
    };
    f32::from_bits(sign | bits)
}
// Input construction and diagnostic export models ONLY; neither rule is presumed native.
fn half(bits: u32, rne: bool) -> u16 {
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 255) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 255 {
        return sign
            | 0x7c00
            | if mant == 0 {
                0
            } else {
                ((mant >> 13) as u16) | 0x0200
            };
    }
    let e = exp - 127;
    if e > 15 {
        return sign | if rne { 0x7c00 } else { 0x7bff };
    }
    if e < -25 {
        return sign;
    }
    let (q, rem, shift) = if e >= -14 {
        ((((e + 15) as u32) << 10) | (mant >> 13), mant & 8191, 13)
    } else {
        let shift = (-e - 1) as u32;
        let sig = mant | 0x80_0000;
        (sig >> shift, sig & ((1 << shift) - 1), shift)
    };
    let mid = 1 << (shift - 1);
    sign | (q + u32::from(rne && (rem > mid || (rem == mid && q & 1 != 0)))) as u16
}
fn rotate<T: Copy>(v: [T; 4], n: usize) -> [T; 4] {
    std::array::from_fn(|i| v[(i + n) % 4])
}
fn exact(out: &mut Vec<Row>, kind: &'static str, name: &str, s: [u16; 4], d: [u16; 4]) {
    for rot in 0..4 {
        let h = rotate(s, rot);
        out.push(Row {
            kind,
            name: format!("{name}-rot{rot}"),
            float: h.map(|x| hfloat(x).to_bits()),
            half: h,
            destination: rotate(d, rot),
        });
    }
}
fn float_pair(out: &mut Vec<Row>, kind: &'static str, name: &str, bits: u32, lo: u16, hi: u16) {
    for rot in 0..2 {
        let f = rotate([bits, bits | 0x8000_0000, bits, bits | 0x8000_0000], rot);
        out.push(Row {
            kind,
            name: format!("{name}-rot{rot}"),
            float: f,
            half: f.map(|x| half(x, true)),
            destination: rotate([lo, lo | 0x8000, hi, hi | 0x8000], rot),
        });
    }
}
fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    for (i, (s, d)) in [
        (
            [0x3a00, 0xc000, 0x3400, 0xc400],
            [0x3400, 0xc400, 0x3a00, 0xc000],
        ),
        (
            [0x3c00, 0xbc00, 0x7bff, 0xfbff],
            [0x3c00, 0xbc00, 0x77ff, 0xf7ff],
        ),
        (
            [0x0400, 0x8400, 0x3555, 0xb555],
            [0x0401, 0x8401, 0x3556, 0xb556],
        ),
        (
            [0x0400, 0x8400, 0x7bff, 0xfbff],
            [0x0400, 0x8400, 0x7bff, 0xfbff],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        exact(&mut out, "anchor", &format!("normal-{i}"), s, d);
    }
    exact(
        &mut out,
        "signed-zero",
        "zero-ties",
        [0, 0x8000, 0, 0x8000],
        [0x8000, 0, 0, 0x8000],
    );
    for (i, (s, d)) in [
        ([1, 0x8001, 0x03ff, 0x83ff], [0, 0x8000, 0x0400, 0x8400]),
        ([0, 0x8000, 0x0400, 0x8400], [1, 0x8001, 0x03ff, 0x83ff]),
        ([2, 0x8002, 0x0200, 0x8200], [1, 0x8001, 0x01ff, 0x81ff]),
        ([1, 0x8001, 0x03ff, 0x83ff], [1, 0x8001, 0x03ff, 0x83ff]),
    ]
    .into_iter()
    .enumerate()
    {
        exact(&mut out, "half-subnormal", &format!("sub-{i}"), s, d);
    }
    for lo in [
        0, 1, 0x03ff, 0x0400, 0x3555, 0x3bff, 0x3c00, 0x3c01, 0x63ff, 0x7bfe,
    ] {
        let mid = ((hfloat(lo) + hfloat(lo + 1)) * 0.5).to_bits();
        for (phase, bits) in [mid - 1, mid, mid + 1].into_iter().enumerate() {
            float_pair(
                &mut out,
                "float-boundary",
                &format!("h{lo:04x}-phase{phase}"),
                bits,
                lo,
                lo + 1,
            );
        }
    }
    for (i, bits) in [
        65520.0f32.to_bits() - 1,
        65520.0f32.to_bits(),
        65520.0f32.to_bits() + 1,
        f32::MAX.to_bits(),
    ]
    .into_iter()
    .enumerate()
    {
        float_pair(
            &mut out,
            "finite-overflow",
            &format!("overflow-{i}"),
            bits,
            0x7bff,
            0x7800,
        );
    }
    // Both sign directions + independent exact-zero ties are essential. Finite
    // nonzero rounded results alone cannot generally identify comparison order.
    for (i, bits) in [
        1,
        0x007f_ffff,
        0x3280_0000,
        0x32ff_ffff,
        0x3300_0000,
        0x3300_0001,
        0x337f_ffff,
        0x3380_0000,
    ]
    .into_iter()
    .enumerate()
    {
        for rot in 0..2 {
            let f = rotate([bits, bits | 0x8000_0000, bits, bits | 0x8000_0000], rot);
            out.push(Row {
                kind: "order-underflow",
                name: format!("tiny-{i}-rot{rot}"),
                float: f,
                half: f.map(|x| half(x, true)),
                destination: rotate([0x8000, 0, 0, 0x8000], rot),
            });
        }
    }
    let nan = [0x7e01, 0xfe2a, 0x7c01, 0xfc03];
    for (i, n) in nan.into_iter().enumerate() {
        exact(
            &mut out,
            "nan-observe",
            &format!("nan-pair-{i}"),
            [n, 0x3800, n, 0xfc00],
            [0xb800, n, 0x7c00, n],
        );
        exact(
            &mut out,
            "nan-observe",
            &format!("nan-matrix-{i}"),
            [n; 4],
            nan,
        );
    }
    for (i, (s, d)) in [
        (
            [0x7c00, 0xfc00, 0x3c00, 0xbc00],
            [0x3c00, 0xbc00, 0x7c00, 0xfc00],
        ),
        (
            [0x7c00, 0xfc00, 0x7c00, 0xfc00],
            [0xfc00, 0x7c00, 0x7c00, 0xfc00],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        exact(&mut out, "infinity-observe", &format!("inf-{i}"), s, d);
    }
    // Low float payload bits deliberately do not fit a half payload. Half inputs
    // remain separately explicit raw ushort4, never an implicit MSL conversion.
    for rot in 0..4 {
        out.push(Row {
            kind: "float-nan-payload",
            name: format!("payload-rot{rot}"),
            float: rotate([0x7fc0_0001, 0xffc5_4021, 0x7f80_0001, 0xff80_0123], rot),
            half: rotate(nan, rot),
            destination: rotate([0x3c00, 0x7e35, 0xfc00, 0x7c02], rot),
        });
    }
    assert_eq!(out.len(), 164);
    assert!(out.len() <= 192);
    out
}
fn specs() -> Vec<Spec> {
    let mut out = Vec::new();
    for precision in [32, 16] {
        for mode in ["default", "replace"] {
            out.push(Spec {
                name: format!("f{precision}-{mode}"),
                precision,
                mode,
                mask: 15,
                tag: 0,
                negative: false,
            });
        }
        for mask in MASKS {
            out.push(Spec {
                name: format!("f{precision}-min-{mask}"),
                precision,
                mode: "min",
                mask,
                tag: 0,
                negative: false,
            });
        }
    }
    out.push(Spec {
        name: "f32-negative-skip".into(),
        precision: 32,
        mode: "min",
        mask: 15,
        tag: 0,
        negative: true,
    });
    for (i, s) in out.iter_mut().enumerate() {
        s.tag = i as u32 + 1;
    }
    assert_eq!(out.len(), 21);
    out
}
fn offset(row: usize, x: usize, lane: usize) -> usize {
    GUARD + row * PITCH + x * 8 + lane * 2
}
fn read16(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes(b[p..p + 2].try_into().unwrap())
}
fn read32(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn put16(b: &mut [u8], p: usize, v: u16) {
    b[p..p + 2].copy_from_slice(&v.to_le_bytes());
}
fn seed(rows: &[Row]) -> Vec<u8> {
    let mut b = vec![POISON; GUARD * 2 + (rows.len() + 2) * PITCH];
    let borders = [
        0x8000, 1, 0x8001, 0x7c00, 0xfc00, 0x7e35, 0xfe17, 0x7c01, 0xfc02, 0x3555,
    ];
    for y in 0..rows.len() + 2 {
        for x in 0..WIDTH {
            for lane in 0..4 {
                let v = if x == 1 && y > 0 && y <= rows.len() {
                    rows[y - 1].destination[lane]
                } else {
                    borders[(y * 12 + x * 4 + lane) % borders.len()]
                };
                put16(&mut b, offset(y, x, lane), v);
            }
        }
    }
    b
}
fn source_float(rows: &[Row]) -> Vec<u8> {
    rows.iter()
        .flat_map(|r| r.float.iter().flat_map(|v| v.to_le_bytes()))
        .collect()
}
fn source_half(rows: &[Row]) -> Vec<u8> {
    rows.iter()
        .flat_map(|r| r.half.iter().flat_map(|v| v.to_le_bytes()))
        .collect()
}
fn guarded(v: &[u8]) -> Vec<u8> {
    [
        vec![POISON; TRACE_GUARD],
        v.to_vec(),
        vec![POISON; TRACE_GUARD],
    ]
    .concat()
}
fn manifest(rows: &[Row]) -> String {
    let cases = rows.iter().enumerate().map(|(i,r)| format!(
        "{{\"id\":{i},\"kind\":\"{}\",\"name\":\"{}\",\"source_f32\":{:?},\"source_f16\":{:?},\"destination_f16\":{:?}}}", r.kind,r.name,r.float,r.half,r.destination)).collect::<Vec<_>>().join(",\n");
    let captures = specs().iter().map(|s| format!(
        "{{\"name\":\"{}\",\"precision\":{},\"mode\":\"{}\",\"mask\":{},\"tag\":{},\"skip\":{},\"negative\":{}}}",
        s.name,s.precision,s.mode,s.mask,s.tag,if s.negative { SKIP as u32 } else { u32::MAX },s.negative)).collect::<Vec<_>>().join(",\n");
    format!("{{\"schema\":1,\"count\":{},\"width\":3,\"height\":{},\"pitch\":256,\"guardBytes\":256,\"traceGuard\":64,\"traceStride\":32,\"cases\":[\n{cases}\n],\"captures\":[\n{captures}\n]}}\n",rows.len(),rows.len()+2)
}
fn write_new(path: &Path, b: &[u8]) -> Result<()> {
    need(b.len() <= 1_048_576, "write size")?;
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .and_then(|mut f| f.write_all(b))
        .map_err(|e| format!("write: {}", e.kind()))
}
fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("metadata: {}", e.kind()))?;
    need(
        m.is_file() && !m.file_type().is_symlink() && m.len() <= 1_048_576,
        "read bounds/type",
    )?;
    fs::read(path).map_err(|e| format!("read: {}", e.kind()))
}
fn generate(root: &Path) -> Result<()> {
    let r = rows();
    for (name, bytes) in [
        ("source-float.bin", source_float(&r)),
        ("source-half.bin", source_half(&r)),
        ("seed.bin", seed(&r)),
        ("cases.json", manifest(&r).into_bytes()),
    ] {
        write_new(&root.join(name), &bytes)?;
    }
    println!(
        "synthetic_rows={} captures=21 command_buffers=22 channels=4 masks={MASKS:?}",
        r.len()
    );
    Ok(())
}
struct Capture {
    upload: Vec<u8>,
    target: Vec<u8>,
    trace: Vec<u8>,
}
fn guards(c: &Capture, count: usize) -> Result<()> {
    let height = count + 2;
    need(
        c.target.len() == GUARD * 2 + height * PITCH,
        "target length",
    )?;
    need(
        c.trace.len() == TRACE_GUARD * 2 + count * TRACE_STRIDE,
        "trace length",
    )?;
    need(
        c.target[..GUARD]
            .iter()
            .chain(c.target[GUARD + height * PITCH..].iter())
            .all(|x| *x == POISON),
        "target guards",
    )?;
    for y in 0..height {
        need(
            c.target[GUARD + y * PITCH + WIDTH * 8..GUARD + (y + 1) * PITCH]
                .iter()
                .all(|x| *x == POISON),
            format!("row padding y={y}"),
        )?;
    }
    need(
        c.trace[..TRACE_GUARD]
            .iter()
            .chain(c.trace[TRACE_GUARD + count * TRACE_STRIDE..].iter())
            .all(|x| *x == POISON),
        "trace guards",
    )
}
fn shader_bits(c: &Capture, row: usize, lane: usize) -> u32 {
    read32(&c.trace, TRACE_GUARD + row * TRACE_STRIDE + 16 + lane * 4)
}
fn expected_anchor(s: &Spec, r: &Row, lane: usize) -> u16 {
    let src = r.half[lane];
    if s.mode == "min" && hfloat(r.destination[lane]) < hfloat(src) {
        r.destination[lane]
    } else {
        src
    }
}
fn expect_word(c: &Capture, row: usize, lane: usize, expected: u16) -> Result<()> {
    need(
        read16(&c.target, offset(row + 1, 1, lane)) == expected,
        format!("anchor row={row} lane={lane}"),
    )
}
// Ordinary validator: NO negative/skipped-row exemption and no NaN canonicalization.
fn validate_capture(c: &Capture, s: &Spec, rows: &[Row], baseline: &[u8]) -> Result<()> {
    need(c.upload == baseline, "pre-render upload readback")?;
    guards(c, rows.len())?;
    for (i, r) in rows.iter().enumerate() {
        let p = TRACE_GUARD + i * TRACE_STRIDE;
        need(
            read32(&c.trace, p) == MAGIC ^ s.tag
                && read32(&c.trace, p + 4) == i as u32
                && read32(&c.trace, p + 8) == s.precision
                && read32(&c.trace, p + 12) == !(i as u32),
            format!("trace-marker row={i}"),
        )?;
        for (lane, bit) in LANES.into_iter().enumerate() {
            let want = if s.precision == 32 {
                r.float[lane]
            } else {
                u32::from(r.half[lane])
            };
            need(
                shader_bits(c, i, lane) == want,
                format!("raw shader source row={i} lane={lane}"),
            )?;
            if r.kind == "anchor" && s.mask & bit != 0 {
                expect_word(c, i, lane, expected_anchor(s, r, lane))?;
            }
        }
    }
    for y in 0..rows.len() + 2 {
        for x in 0..WIDTH {
            for (lane, bit) in LANES.into_iter().enumerate() {
                if x != 1 || y == 0 || y == rows.len() + 1 || s.mask & bit == 0 {
                    let p = offset(y, x, lane);
                    need(
                        c.target[p..p + 2] == baseline[p..p + 2],
                        format!("preservation y={y} x={x} lane={lane}"),
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn state(s: &Spec) -> Vec<u32> {
    let mut v = if s.mode == "default" {
        vec![15, 0, 0, 0, 1, 0, 1, 0, s.precision]
    } else {
        vec![
            s.mask,
            u32::from(s.mode == "min"),
            3,
            3,
            1,
            1,
            1,
            1,
            s.precision,
        ]
    };
    v.extend([115, 1, 0, 0, 1, 1, 1, 1, 3, 1]);
    v
}
fn receipt_text(text: &str, count: usize, specs: &[Spec]) -> Result<()> {
    let lines: Vec<_> = text.lines().collect();
    need(
        lines.len() == specs.len() + 2 && lines[0] == RECEIPT_HEADER,
        "receipt coverage/header",
    )?;
    for (i, line) in lines[1..].iter().enumerate() {
        let f: Vec<_> = line.split('\t').collect();
        need(f.len() == 28, "receipt fields")?;
        let n = |j: usize| {
            f[j].parse::<u32>()
                .map_err(|_| "receipt integer".to_string())
        };
        need(
            n(0)? == i as u32
                && f[1]
                    == if i == 0 {
                        "baseline"
                    } else {
                        &specs[i - 1].name
                    },
            "receipt identity/order",
        )?;
        need(n(2)? == 4 && n(3)? == 0, "command completion/error")?;
        let start: f64 = f[4].parse().map_err(|_| "GPU time")?;
        let end: f64 = f[5].parse().map_err(|_| "GPU time")?;
        need(
            start.is_finite() && end.is_finite() && start >= 0.0 && end >= start,
            "GPU timing range",
        )?;
        need(
            n(6)? == 115 && n(7)? == WIDTH as u32 && n(8)? == (count + 2) as u32,
            "texture getters",
        )?;
        let want = if i == 0 {
            vec![0; 19]
        } else {
            state(&specs[i - 1])
        };
        for (j, v) in want.into_iter().enumerate() {
            need(n(9 + j)? == v, "pipeline getters")?;
        }
    }
    Ok(())
}
// Eight competing MIN models: comparison order x RNE/RTZ export x tie direction.
// Finite inputs only. Not a production implementation or an empirical conclusion.
fn model(source: f32, destination: u16, id: usize) -> u16 {
    let rne = id & 2 == 0;
    let source_tie = id & 1 == 0;
    let s = if id < 4 {
        source
    } else {
        hfloat(half(source.to_bits(), rne))
    };
    let d = hfloat(destination);
    half(
        if s < d || (s == d && source_tie) {
            s.to_bits()
        } else {
            d.to_bits()
        },
        rne,
    )
}
fn classify(h: u16) -> &'static str {
    match (h & 0x7c00, h & 0x03ff) {
        (0, 0) => {
            if h & 0x8000 == 0 {
                "+zero"
            } else {
                "-zero"
            }
        }
        (0, _) => "subnormal",
        (0x7c00, 0) => {
            if h & 0x8000 == 0 {
                "+inf"
            } else {
                "-inf"
            }
        }
        (0x7c00, _) => {
            if h & 0x0200 == 0 {
                "sNaN-bits"
            } else {
                "qNaN-bits"
            }
        }
        _ => "normal",
    }
}
fn returned(c: &Capture, s: &Spec, i: usize, lane: usize) -> f32 {
    let b = shader_bits(c, i, lane);
    if s.precision == 32 {
        f32::from_bits(b)
    } else {
        hfloat(b as u16)
    }
}
struct Checked {
    json: String,
    table: String,
    wrong_expected_rejected: bool,
}
fn acquire(root: &Path, expected: &str, source: &Path) -> Result<Checked> {
    bounds(root)?;
    binding(root, expected, source)?;
    need(
        read_bounded(&root.join("complete.txt"))? == COMPLETE.as_bytes(),
        "native completion",
    )?;
    need(
        read_bounded(&root.join("sdk-enums.tsv"))? == ENUMS.as_bytes(),
        "SDK enum getters",
    )?;
    need(
        read_bounded(&root.join("compiler.tsv"))? == COMPILER.as_bytes(),
        "compiler option getters/profile scope",
    )?;
    let r = rows();
    let sp = specs();
    let baseline = seed(&r);
    for (name, b) in [
        ("source-float.bin", source_float(&r)),
        ("source-half.bin", source_half(&r)),
        ("seed.bin", baseline.clone()),
        ("cases.json", manifest(&r).into_bytes()),
        ("source-float-after.bin", guarded(&source_float(&r))),
        ("source-half-after.bin", guarded(&source_half(&r))),
        ("seed-after.bin", baseline.clone()),
        ("baseline-target.bin", baseline.clone()),
    ] {
        need(
            read_bounded(&root.join(name))? == b,
            format!("input/baseline integrity: {name}"),
        )?;
    }
    receipt_text(
        &String::from_utf8(read_bounded(&root.join("receipts.tsv"))?)
            .map_err(|_| "receipt UTF-8")?,
        r.len(),
        &sp,
    )?;
    let mut cap = Vec::new();
    for s in &sp {
        cap.push(Capture {
            upload: read_bounded(&root.join(format!("{}-upload.bin", s.name)))?,
            target: read_bounded(&root.join(format!("{}-target.bin", s.name)))?,
            trace: read_bounded(&root.join(format!("{}-trace.bin", s.name)))?,
        });
    }
    for (s, c) in sp.iter().zip(&cap).filter(|(s, _)| !s.negative) {
        validate_capture(c, s, &r, &baseline).map_err(|e| format!("{}: {e}", s.name))?;
    }
    let mut finite_control = 0;
    let mut mask_lanes = 0;
    let mut nonfinite_mask_differences = 0;
    for base in [0, 10] {
        for i in 0..r.len() {
            for lane in 0..4 {
                let p = offset(i + 1, 1, lane);
                if returned(&cap[base], &sp[base], i, lane).is_finite() {
                    need(
                        cap[base].target[p..p + 2] == cap[base + 1].target[p..p + 2],
                        "default vs explicit replacement",
                    )?;
                    finite_control += 1;
                }
            }
        }
        for idx in base + 2..base + 10 {
            for (i, row) in r.iter().enumerate() {
                for (lane, bit) in LANES.into_iter().enumerate() {
                    if sp[idx].mask & bit == 0 {
                        continue;
                    }
                    let p = offset(i + 1, 1, lane);
                    let equal = cap[idx].target[p..p + 2] == cap[base + 9].target[p..p + 2];
                    if returned(&cap[idx], &sp[idx], i, lane).is_finite()
                        && hfloat(row.destination[lane]).is_finite()
                    {
                        need(equal, "enabled finite mask vs full-mask")?;
                        mask_lanes += 1;
                    } else {
                        nonfinite_mask_differences += usize::from(!equal);
                    }
                }
            }
        }
    }
    let n = &cap[20];
    need(
        validate_capture(n, &sp[20], &r, &baseline).err()
            == Some(format!("trace-marker row={SKIP}")),
        "native skipped-row negative not isolated",
    )?;
    need(n.upload == baseline, "negative upload")?;
    guards(n, r.len())?;
    for i in 0..r.len() {
        let t = TRACE_GUARD + i * TRACE_STRIDE;
        if i == SKIP {
            need(
                n.trace[t..t + TRACE_STRIDE].iter().all(|b| *b == POISON),
                "skipped trace",
            )?;
            let p = offset(i + 1, 1, 0);
            need(
                n.target[p..p + 8] == baseline[p..p + 8]
                    && n.target[p..p + 8] != cap[9].target[p..p + 8],
                "skipped target",
            )?;
        } else {
            need(
                read32(&n.trace, t) == MAGIC ^ sp[20].tag
                    && n.trace[t + 4..t + TRACE_STRIDE] == cap[9].trace[t + 4..t + TRACE_STRIDE],
                "negative other trace",
            )?;
            for lane in 0..4 {
                if returned(n, &sp[20], i, lane).is_finite()
                    && hfloat(r[i].destination[lane]).is_finite()
                {
                    let p = offset(i + 1, 1, lane);
                    need(
                        n.target[p..p + 2] == cap[9].target[p..p + 2],
                        "negative other finite word",
                    )?;
                }
            }
        }
    }
    for y in 0..r.len() + 2 {
        for x in 0..WIDTH {
            if x != 1 || y == 0 || y == r.len() + 1 {
                let p = offset(y, x, 0);
                need(n.target[p..p + 8] == baseline[p..p + 8], "negative border")?;
            }
        }
    }
    let wrong = expected_anchor(&sp[9], &r[0], 0) ^ 1;
    let wrong_expected_rejected =
        expect_word(&cap[9], 0, 0, wrong).err() == Some("anchor row=0 lane=0".into());
    need(wrong_expected_rejected, "wrong expected passed")?;
    let mut table=String::from("capture\trow\tkind\tcase\tlane\tinput_f32\tinput_f16\treturned_bits\tdestination_f16\tactual_f16\tclass\tRNE\tRTZ\tmodels_0_to_7\n");
    let mut mismatches = [[0usize; 8]; 2];
    let mut totals = [0usize; 2];
    let mut export_mismatch = [[0usize; 2]; 2];
    let mut export_count = [0usize; 2];
    let mut disagreements = [[0usize; 8]; 8];
    for (pi, base) in [0, 10].into_iter().enumerate() {
        // Export is measured separately with disabled blending, not deduced from MIN.
        for idx in [base, base + 1, base + 9] {
            let s = &sp[idx];
            let c = &cap[idx];
            for (i, row) in r.iter().enumerate() {
                for (lane, channel) in ["R", "G", "B", "A"].into_iter().enumerate() {
                    let src = returned(c, s, i, lane);
                    let dst = row.destination[lane];
                    let actual = read16(&c.target, offset(i + 1, 1, lane));
                    let mut pred = "nonfinite-observe-only".to_string();
                    let mut ex = ["NA".to_string(), "NA".to_string()];
                    if src.is_finite() {
                        let values = [half(src.to_bits(), true), half(src.to_bits(), false)];
                        ex = values.map(|v| format!("{v:04x}"));
                        if idx == base + 1 {
                            export_count[pi] += 1;
                            for j in 0..2 {
                                export_mismatch[pi][j] += usize::from(values[j] != actual);
                            }
                        }
                        if hfloat(dst).is_finite() {
                            let m: [u16; 8] = std::array::from_fn(|id| model(src, dst, id));
                            pred = m
                                .iter()
                                .map(|v| format!("{v:04x}"))
                                .collect::<Vec<_>>()
                                .join(",");
                            if idx == base + 9 {
                                totals[pi] += 1;
                                for id in 0..8 {
                                    mismatches[pi][id] += usize::from(m[id] != actual);
                                }
                                if pi == 0 {
                                    for a in 0..8 {
                                        for b in 0..8 {
                                            disagreements[a][b] += usize::from(m[a] != m[b]);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    table.push_str(&format!("{}\t{i}\t{}\t{}\t{channel}\t{:08x}\t{:04x}\t{:08x}\t{dst:04x}\t{actual:04x}\t{}\t{}\t{}\t{pred}\n",s.name,row.kind,row.name,row.float[lane],row.half[lane],shader_bits(c,i,lane),classify(actual),ex[0],ex[1]));
                }
            }
        }
    }
    let json=format!(concat!("{{\"schema\":1,\"status\":\"OBSERVATIONS_COMPLETE\",\"rows\":{},\"channels\":4,\"positive_captures\":20,\"command_buffers\":22,",
        "\"preupload_final_trace_guards_checked\":true,\"disabled_lanes_exact\":true,\"skipped_row_negative_rejected\":true,\"wrong_expected_rejected\":true,",
        "\"finite_replace_control_lanes\":{},\"finite_mask_comparisons\":{},\"nonfinite_mask_differences_diagnostic\":{},",
        "\"export_lanes_f32_f16\":{:?},\"export_mismatches_RNE_RTZ_f32_f16\":{:?},\"finite_min_lanes_f32_f16\":{:?},\"min_model_mismatches_f32_f16\":{:?},\"float_model_pairwise_disagreements\":{:?},",
        "\"model_order\":[\"compare-RNE-source-tie\",\"compare-RNE-dest-tie\",\"compare-RTZ-source-tie\",\"compare-RTZ-dest-tie\",\"quantize-RNE-source-tie\",\"quantize-RNE-dest-tie\",\"quantize-RTZ-source-tie\",\"quantize-RTZ-dest-tie\"],",
        "\"scope\":\"sampled consistency only; nonfinite payloads are observations; no production adoption or universal rule\",\"factor_ignorance_measured\":false,\"cpu_generated_gpu_outputs\":false}}\n"),
        r.len(),finite_control,mask_lanes,nonfinite_mask_differences,export_count,export_mismatch,totals,mismatches,disagreements);
    Ok(Checked {
        json,
        table,
        wrong_expected_rejected,
    })
}

// Integrity/binding helpers and tests follow. SHA-256 is not a numerical oracle.
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
    let r = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|_| "git execution")?;
    need(r.status.success(), "git binding command")?;
    Ok(String::from_utf8(r.stdout)
        .map_err(|_| "git UTF-8")?
        .trim()
        .into())
}
fn source_pins(source: &Path, expected: &str) -> Result<String> {
    need(
        expected.len() == 40
            && expected
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "expected head syntax",
    )?;
    need(
        git(source, &["rev-parse", "HEAD"])? == expected,
        "head binding",
    )?;
    let mut s = String::from("path\tbytes\tsha256\tgit_blob\n");
    for path in SOURCE {
        let b = read_bounded(&source.join(path))?;
        let blob = git(source, &["rev-parse", &format!("{expected}:{path}")])?;
        need(
            blob == git(source, &["hash-object", "--", path])?,
            "source blob binding",
        )?;
        // Compare raw working bytes to the reviewed blob, not just filter-aware hash-object.
        let raw = Command::new("git")
            .current_dir(source)
            .args(["show", &format!("{expected}:{path}")])
            .output()
            .map_err(|_| "git source read")?;
        need(
            raw.status.success() && raw.stdout == b,
            "raw source bytes differ from reviewed commit",
        )?;
        s.push_str(&format!("{path}\t{}\t{}\t{blob}\n", b.len(), sha256(&b)));
    }
    Ok(s)
}
fn binding_text(expected: &str) -> String {
    format!("reviewed_head={expected}\ndispatch_head={expected}\nchecked_head={expected}\n")
}
fn binding(root: &Path, expected: &str, source: &Path) -> Result<()> {
    need(
        read_bounded(&root.join("binding.txt"))? == binding_text(expected).as_bytes(),
        "binding receipt",
    )?;
    need(
        read_bounded(&root.join("source-pins.tsv"))? == source_pins(source, expected)?.as_bytes(),
        "source pins",
    )
}
fn allowed() -> Vec<String> {
    let mut names = [
        "source-float.bin",
        "source-half.bin",
        "seed.bin",
        "cases.json",
        "binding.txt",
        "source-pins.tsv",
        "baseline.txt",
        "tests.txt",
        "command-exit.json",
        "sdk-enums.tsv",
        "compiler.tsv",
        "receipts.tsv",
        "gpu.json",
        "complete.txt",
        "baseline-target.bin",
        "source-float-after.bin",
        "source-half-after.bin",
        "seed-after.bin",
        "observations.tsv",
        "validation.json",
        "negative.json",
        "SHA256SUMS",
    ]
    .map(str::to_string)
    .to_vec();
    for s in specs() {
        for suffix in ["upload", "target", "trace"] {
            names.push(format!("{}-{suffix}.bin", s.name));
        }
    }
    names.sort();
    names
}
fn bounds(root: &Path) -> Result<()> {
    let m = fs::symlink_metadata(root).map_err(|_| "result directory")?;
    need(
        m.is_dir() && !m.file_type().is_symlink(),
        "result directory type",
    )?;
    let mut count = 0;
    let mut total = 0;
    let names = allowed();
    for e in fs::read_dir(root).map_err(|_| "result listing")? {
        let e = e.map_err(|_| "result entry")?;
        let n = e.file_name().into_string().map_err(|_| "result name")?;
        need(
            names.binary_search(&n).is_ok(),
            "artifact outside exact allowlist",
        )?;
        let m = fs::symlink_metadata(e.path()).map_err(|_| "result metadata")?;
        need(
            m.is_file() && !m.file_type().is_symlink() && m.len() <= 1_048_576,
            "artifact type/size",
        )?;
        total += m.len();
        count += 1;
    }
    need(count <= 85 && total <= LIMIT, "artifact count/total")
}
fn seal_text(root: &Path) -> Result<String> {
    bounds(root)?;
    let mut entries = BTreeMap::new();
    for e in fs::read_dir(root).map_err(|_| "seal listing")? {
        let e = e.map_err(|_| "seal entry")?;
        let n = e.file_name().into_string().map_err(|_| "seal name")?;
        if n != "SHA256SUMS" {
            entries.insert(n, read_bounded(&e.path())?);
        }
    }
    Ok(entries
        .into_iter()
        .map(|(n, b)| format!("{}  {n}\n", sha256(&b)))
        .collect())
}
fn verify(root: &Path, expected: &str, source: &Path) -> Result<()> {
    let c = acquire(root, expected, source)?;
    need(
        read_bounded(&root.join("validation.json"))? == c.json.as_bytes(),
        "validation replay",
    )?;
    need(
        read_bounded(&root.join("observations.tsv"))? == c.table.as_bytes(),
        "observation replay",
    )?;
    need(
        c.wrong_expected_rejected
            && read_bounded(&root.join("negative.json"))? == NEGATIVE.as_bytes(),
        "negative receipt",
    )?;
    need(
        read_bounded(&root.join("command-exit.json"))? == EXIT_OK.as_bytes(),
        "run exit/phase",
    )?;
    for name in allowed() {
        read_bounded(&root.join(name))?;
    }
    need(
        read_bounded(&root.join("SHA256SUMS"))? == seal_text(root)?.as_bytes(),
        "artifact seal",
    )
}
fn partial_failure_exit(b: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(b) else {
        return false;
    };
    let Some(s) = s.strip_prefix("{\"exit\":") else {
        return false;
    };
    let Some((code, phase)) = s.split_once(",\"phase\":\"") else {
        return false;
    };
    let Some(phase) = phase.strip_suffix("\"}\n") else {
        return false;
    };
    matches!(code.parse::<u32>(), Ok(1..=255))
        && [
            "build",
            "tests",
            "bind",
            "generate",
            "native",
            "check",
            "expected_negative",
            "complete",
        ]
        .contains(&phase)
}
fn upload_guard(root: &Path, expected: &str, source: &Path) -> Result<()> {
    bounds(root)?;
    binding(root, expected, source)?;
    need(
        read_bounded(&root.join("SHA256SUMS"))? == seal_text(root)?.as_bytes(),
        "final upload seal",
    )?;
    let exit = read_bounded(&root.join("command-exit.json"))?;
    if exit == EXIT_OK.as_bytes() {
        verify(root, expected, source)?;
        println!("SEALED_COMPLETE_OBSERVATIONS_NO_PRODUCTION_ADOPTION");
    } else {
        need(partial_failure_exit(&exit), "partial failure exit receipt")?;
        println!("SEALED_PARTIAL_FAILURE_NOT_NATIVE_PASS");
    }
    Ok(())
}
fn main() {
    let a: Vec<_> = env::args().collect();
    let result = (|| -> Result<i32> {
        need(
            a.len() == 3 || a.len() == 5,
            "expected MODE OUTPUT [EXPECTED_HEAD SOURCE_ROOT]",
        )?;
        let root = Path::new(&a[2]);
        match a[1].as_str() {
            "generate" if a.len() == 3 => {
                generate(root)?;
            }
            "bounds" if a.len() == 3 => {
                bounds(root)?;
            }
            "seal" if a.len() == 3 => {
                write_new(&root.join("SHA256SUMS"), seal_text(root)?.as_bytes())?;
                bounds(root)?;
            }
            "bind" if a.len() == 5 => {
                let pins = source_pins(Path::new(&a[4]), &a[3])?;
                write_new(&root.join("binding.txt"), binding_text(&a[3]).as_bytes())?;
                write_new(&root.join("source-pins.tsv"), pins.as_bytes())?;
            }
            "check" if a.len() == 5 => {
                let c = acquire(root, &a[3], Path::new(&a[4]))?;
                write_new(&root.join("observations.tsv"), c.table.as_bytes())?;
                write_new(&root.join("validation.json"), c.json.as_bytes())?;
            }
            "negative" if a.len() == 5 => {
                let c = acquire(root, &a[3], Path::new(&a[4]))?;
                need(
                    c.wrong_expected_rejected,
                    "wrong expected negative did not reject",
                )?;
                write_new(&root.join("negative.json"), NEGATIVE.as_bytes())?;
                println!("WRONG_EXPECTED_REJECTED: unchanged native readback, ordinary anchor comparison");
                return Ok(2);
            }
            "verify" if a.len() == 5 => {
                verify(root, &a[3], Path::new(&a[4]))?;
            }
            "upload-guard" if a.len() == 5 => {
                upload_guard(root, &a[3], Path::new(&a[4]))?;
            }
            _ => return Err("invalid mode/arguments".into()),
        }
        Ok(0)
    })();
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("NOT_QUALIFIED: {e}");
            std::process::exit(3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // CPU fixtures are test-only, kept in memory, and NEVER written as observations.
    fn fixture(s: &Spec, r: &[Row]) -> Capture {
        let mut c = Capture {
            upload: seed(r),
            target: seed(r),
            trace: vec![POISON; TRACE_GUARD * 2 + r.len() * TRACE_STRIDE],
        };
        for (i, row) in r.iter().enumerate() {
            let p = TRACE_GUARD + i * TRACE_STRIDE;
            let mut words = vec![MAGIC ^ s.tag, i as u32, s.precision, !(i as u32)];
            words.extend(if s.precision == 32 {
                row.float
            } else {
                row.half.map(u32::from)
            });
            for (k, w) in words.into_iter().enumerate() {
                c.trace[p + k * 4..p + k * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            for (lane, bit) in LANES.into_iter().enumerate() {
                if row.kind == "anchor" && s.mask & bit != 0 {
                    put16(
                        &mut c.target,
                        offset(i + 1, 1, lane),
                        expected_anchor(s, row, lane),
                    );
                }
            }
        }
        c
    }
    #[test]
    fn finite_half_roundtrip_exhaustive_cpu_only() {
        for h in 0..=u16::MAX {
            if h & 0x7c00 != 0x7c00 {
                assert_eq!(half(hfloat(h).to_bits(), true), h);
                assert_eq!(half(hfloat(h).to_bits(), false), h);
            }
        }
    }
    #[test]
    fn conversion_midpoints_and_extremes() {
        for (f, rne, rtz) in [
            (1.00048828125f32, 0x3c00, 0x3c00),
            (1.00146484375, 0x3c02, 0x3c01),
            (65520.0, 0x7c00, 0x7bff),
        ] {
            assert_eq!(half(f.to_bits(), true), rne);
            assert_eq!(half(f.to_bits(), false), rtz);
            assert_eq!(half((-f).to_bits(), true), rne | 0x8000);
        }
        assert_eq!(half(0x3300_0000, true), 0);
        assert_eq!(half(0x3300_0001, true), 1);
        assert_eq!(half(0x3300_0001, false), 0);
        assert_eq!(half(0x8000_0001, true), 0x8000);
    }
    #[test]
    fn four_channel_plan_and_budget() {
        let r = rows();
        let s = specs();
        assert_eq!(r.len(), 164);
        assert_eq!(s.len() + 1, 22);
        assert_eq!(allowed().len(), 85);
        assert_eq!(manifest(&r), manifest(&rows()));
        assert_eq!(source_float(&r).len(), r.len() * 16);
        assert_eq!(source_half(&r).len(), r.len() * 8);
        let raw = s.len() * (seed(&r).len() * 2 + TRACE_GUARD * 2 + r.len() * TRACE_STRIDE);
        assert!(raw < 2_000_000); // leaves >2 MiB for manifests, tables, and all receipts
        for base in [0, 10] {
            assert_eq!(s[base + 9].mask, 15);
            assert_eq!(
                s[base + 2..base + 10]
                    .iter()
                    .map(|x| x.mask)
                    .collect::<Vec<_>>(),
                MASKS
            );
        }
        for spec in &s {
            validate_capture(&fixture(spec, &r), spec, &r, &seed(&r)).unwrap();
        }
    }
    #[test]
    fn special_cases_reach_every_channel() {
        let r = rows();
        for lane in 0..4 {
            for value in [
                0, 0x8000, 1, 0x8001, 0x03ff, 0x83ff, 0x0400, 0x8400, 0x7bff, 0xfbff, 0x7c00,
                0xfc00, 0x7e01, 0xfe2a, 0x7c01, 0xfc03,
            ] {
                assert!(
                    r.iter().any(|x| x.half[lane] == value),
                    "source lane={lane} value={value:x}"
                );
                assert!(
                    r.iter().any(|x| x.destination[lane] == value),
                    "destination lane={lane} value={value:x}"
                );
            }
            for s in [0, 0x8000] {
                for d in [0, 0x8000] {
                    assert!(r
                        .iter()
                        .any(|x| x.half[lane] == s && x.destination[lane] == d));
                }
            }
        }
    }
    #[test]
    fn min_is_not_max_and_destination_ties_exist() {
        let r = rows();
        let s = &specs()[9];
        assert_eq!(expected_anchor(s, &r[0], 0), 0x3400);
        assert_eq!(expected_anchor(s, &r[0], 1), 0xc400);
        assert!(r
            .iter()
            .filter(|x| x.kind == "anchor")
            .any(|x| x.half == x.destination));
    }
    #[test]
    fn all_eight_float_models_have_real_witnesses() {
        let r = rows();
        for a in 0..8 {
            for b in a + 1..8 {
                assert!(
                    r.iter().any(|x| (0..4).any(|lane| {
                        let f = f32::from_bits(x.float[lane]);
                        f.is_finite()
                            && hfloat(x.destination[lane]).is_finite()
                            && model(f, x.destination[lane], a) != model(f, x.destination[lane], b)
                    })),
                    "models {a} and {b} not distinguishable"
                );
            }
        }
        // MIN: a positive tiny source vs negative zero exposes source-tie Q-first;
        // negative tiny vs positive zero exposes destination-tie Q-first.
        assert_eq!(model(f32::from_bits(0x3280_0000), 0x8000, 0), 0x8000);
        assert_eq!(model(f32::from_bits(0x3280_0000), 0x8000, 4), 0);
        assert_eq!(model(f32::from_bits(0xb280_0000), 0, 1), 0x8000);
        assert_eq!(model(f32::from_bits(0xb280_0000), 0, 5), 0);
    }
    #[test]
    fn ordinary_validator_rejects_skipped_row() {
        let r = rows();
        let s = &specs()[20];
        let mut c = fixture(s, &r);
        c.trace[TRACE_GUARD + SKIP * TRACE_STRIDE..TRACE_GUARD + (SKIP + 1) * TRACE_STRIDE]
            .fill(POISON);
        assert_eq!(
            validate_capture(&c, s, &r, &seed(&r)).unwrap_err(),
            "trace-marker row=2"
        );
    }
    #[test]
    fn wrong_expected_uses_unchanged_capture() {
        let r = rows();
        let s = &specs()[9];
        let c = fixture(s, &r);
        let before = sha256(&c.target);
        validate_capture(&c, s, &r, &seed(&r)).unwrap();
        assert_eq!(
            expect_word(&c, 0, 0, expected_anchor(s, &r[0], 0) ^ 1).unwrap_err(),
            "anchor row=0 lane=0"
        );
        assert_eq!(sha256(&c.target), before);
    }
    #[test]
    fn strict_lengths_upload_padding_borders_and_traces() {
        let r = rows();
        let s = &specs()[9];
        for p in [
            0,
            GUARD - 1,
            GUARD + WIDTH * 8,
            offset(0, 1, 0),
            offset(1, 0, 2),
            offset(r.len() + 1, 1, 3),
            seed(&r).len() - 1,
            offset(1, 1, 0),
        ] {
            let mut c = fixture(s, &r);
            c.target[p] ^= 1;
            assert!(validate_capture(&c, s, &r, &seed(&r)).is_err());
        }
        let mut c = fixture(s, &r);
        c.upload[GUARD + PITCH + 8] ^= 1;
        assert_eq!(
            validate_capture(&c, s, &r, &seed(&r)).unwrap_err(),
            "pre-render upload readback"
        );
        for trace in [false, true] {
            for append in [false, true] {
                let mut c = fixture(s, &r);
                let b = if trace { &mut c.trace } else { &mut c.target };
                if append {
                    b.push(0);
                } else {
                    b.pop();
                }
                assert!(validate_capture(&c, s, &r, &seed(&r)).is_err());
            }
        }
        for p in [0, TRACE_GUARD + 16, TRACE_GUARD + r.len() * TRACE_STRIDE] {
            let mut c = fixture(s, &r);
            c.trace[p] ^= 1;
            assert!(validate_capture(&c, s, &r, &seed(&r)).is_err());
        }
    }
    #[test]
    fn every_disabled_lane_preserves_nan_payload_bits() {
        let r = rows();
        for s in specs()
            .into_iter()
            .filter(|s| s.mode == "min" && !s.negative)
        {
            for (lane, bit) in LANES.into_iter().enumerate() {
                if s.mask & bit == 0 {
                    let i = r
                        .iter()
                        .position(|x| x.destination[lane] == 0x7c01)
                        .unwrap();
                    let mut c = fixture(&s, &r);
                    let p = offset(i + 1, 1, lane);
                    put16(&mut c.target, p, 0x7e01);
                    assert!(validate_capture(&c, &s, &r, &seed(&r))
                        .unwrap_err()
                        .starts_with("preservation"));
                }
            }
        }
    }
    fn receipts() -> String {
        let mut text = format!("{RECEIPT_HEADER}\n");
        for i in 0..22 {
            let sp = specs();
            let name = if i == 0 { "baseline" } else { &sp[i - 1].name };
            let v = if i == 0 {
                vec![0; 19]
            } else {
                state(&sp[i - 1])
            };
            text.push_str(&format!(
                "{i}\t{name}\t4\t0\t0\t0\t115\t3\t{}\t{}\n",
                rows().len() + 2,
                v.iter().map(u32::to_string).collect::<Vec<_>>().join("\t")
            ));
        }
        text
    }
    #[test]
    fn receipt_mutations_reject_wrong_enum_mask_factor_and_completion() {
        let text = receipts();
        receipt_text(&text, rows().len(), &specs()).unwrap();
        for column in [
            0, 2, 3, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
            26, 27,
        ] {
            let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
            let mut f: Vec<String> = lines[11].split('\t').map(str::to_string).collect();
            f[column] = "99".into();
            lines[11] = f.join("\t");
            assert!(
                receipt_text(&(lines.join("\n") + "\n"), rows().len(), &specs()).is_err(),
                "column {column}"
            );
        }
        assert!(receipt_text(
            &text.replacen("\t0\t0\t115", "\tNaN\t0\t115", 1),
            rows().len(),
            &specs()
        )
        .is_err());
        assert!(receipt_text(&(text + "0\tbaseline\n"), rows().len(), &specs()).is_err());
    }
    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(&[b'a'; 1000]),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
    }
    #[test]
    fn partial_failure_can_never_masquerade_as_success() {
        assert!(partial_failure_exit(b"{\"exit\":3,\"phase\":\"native\"}\n"));
        assert!(!partial_failure_exit(EXIT_OK.as_bytes()));
        for b in [
            "{\"exit\":0,\"phase\":\"native\"}\n",
            "{\"exit\":-1,\"phase\":\"native\"}\n",
            "{\"exit\":3,\"phase\":\"unreviewed\"}\n",
            "{\"exit\":3,\"phase\":\"native\"}\nextra",
        ] {
            assert!(!partial_failure_exit(b.as_bytes()));
        }
    }
}
