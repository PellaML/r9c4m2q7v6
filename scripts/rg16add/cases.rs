// Standalone synthetic inputs, raw-byte validator, and DIAGNOSTIC hypotheses.
// No native interop, dependencies, downloads, or CPU-generated GPU output path.
#![forbid(unsafe_code)]

use std::{env, fs, io::Write, path::Path};

const WIDTH: usize = 3;
const PITCH: usize = 256;
const GUARD: usize = 256;
const TRACE_GUARD: usize = 64;
const POISON: u8 = 0xcd;
const MAGIC: u32 = 0x6164_6400;
const SKIP: usize = 2;
const RECEIPT_HEADER: &str = "tag\tname\tstatus\terror\tgpu_start\tgpu_end\tformat\twidth\theight\tmask\tblending\trgb_op\talpha_op\tsrc_rgb\tdst_rgb\tsrc_alpha\tdst_alpha\treturn_bits";

#[derive(Clone, Debug)]
struct Row {
    kind: &'static str,
    name: String,
    source: [u32; 2],
    destination: [u16; 2],
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

fn require(ok: bool, message: impl Into<String>) -> Result<(), String> {
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

// Used to construct synthetic inputs, exact normal anchors, and named competing
// hypotheses ONLY. Neither RNE nor RTZ is presumed to describe native ADD.
fn half(bits: u32, rne: bool) -> u16 {
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 255) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 255 {
        return sign | 0x7c00 | if mant == 0 { 0 } else { ((mant >> 13) as u16) | 0x0200 };
    }
    let e = exp - 127;
    if e > 15 {
        return sign | if rne { 0x7c00 } else { 0x7bff };
    }
    if e < -25 {
        return sign;
    }
    let (q, remainder, shift) = if e >= -14 {
        ((((e + 15) as u32) << 10) | (mant >> 13), mant & 8191, 13)
    } else {
        let shift = (-e - 1) as u32;
        let sig = mant | 0x80_0000;
        (sig >> shift, sig & ((1 << shift) - 1), shift)
    };
    let midpoint = 1 << (shift - 1);
    let increment = rne && (remainder > midpoint || (remainder == midpoint && q & 1 != 0));
    sign | (q + u32::from(increment)) as u16
}

fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    let mut add = |kind, name: String, source: [u32; 2], destination| {
        out.push(Row { kind, name, source, destination });
    };
    for (i, (s, d)) in [
        ([0.75f32, -2.0], [0.25f32, -4.0]),
        ([0.25, -4.0], [0.75, -2.0]),
        ([2.0, 0.125], [1.0, -0.5]),
        ([-8.0, 8.0], [-2.0, 2.0]),
        ([1.0, 1.0], [1.0, 1.0]),
        ([-1.0, -1.0], [-1.0, -1.0]),
        ([65504.0, -65504.0], [32752.0, -32752.0]),
        ([0.03125, 1.5], [0.0625, 2.0]),
    ]
    .into_iter()
    .enumerate()
    {
        add("anchor", format!("normal-{i}"), s.map(f32::to_bits), d.map(|x| half(x.to_bits(), true)));
    }
    for s in [0u16, 0x8000] {
        for d in [0u16, 0x8000] {
            add(
                "signed-zero",
                format!("zero-{s:04x}-{d:04x}"),
                [hfloat(s).to_bits(), hfloat(d).to_bits()],
                [d, s],
            );
        }
    }
    for lo in [0u16, 1, 0x03ff, 0x0400, 0x3555, 0x3800, 0x3bff, 0x3c00, 0x4000, 0x63ff, 0x7bfe] {
        let midpoint = (hfloat(lo) + hfloat(lo + 1)) * 0.5;
        for (phase, bits) in
            [midpoint.to_bits() - 1, midpoint.to_bits(), midpoint.to_bits() + 1].into_iter().enumerate()
        {
            for (side, dst) in
                [("source-above", [lo, (lo + 1) | 0x8000]), ("destination-above", [lo + 1, lo | 0x8000])]
            {
                add(
                    "half-boundary",
                    format!("h{lo:04x}-ulp{phase}-{side}"),
                    [bits, bits | 0x8000_0000],
                    dst,
                );
            }
        }
    }
    for (i, (s, d)) in [
        ([1, 0x8000_0001], [0, 0x8000]),
        ([0x007f_ffff, 0x807f_ffff], [1, 0x8001]),
        ([0x3380_0000, 0xb380_0000], [0, 0x8000]),
        ([0, 0x8000_0000], [0x8001, 1]),
    ]
    .into_iter()
    .enumerate()
    {
        add("subnormal", format!("subnormal-{i}"), s, d);
    }
    for (i, bits) in [65520.0f32.to_bits() - 1, 65520.0f32.to_bits(), 65520.0f32.to_bits() + 1]
        .into_iter()
        .enumerate()
    {
        add(
            "finite-overflow",
            format!("overflow-tie-{i}"),
            [bits, bits | 0x8000_0000],
            [0x7bff, 0xfbff],
        );
    }
    for (i, (s, d)) in [
        ([0x7f80_0000, 0xff80_0000], [0x3c00, 0xbc00]),
        ([0x3f80_0000, 0xbf80_0000], [0x7c00, 0xfc00]),
        ([0x7f80_0000, 0xff80_0000], [0xfc00, 0x7c00]),
        ([0x7fc0_2000, 0xffc5_4000], [0x3800, 0xb800]),
        ([0x3f00_0000, 0xbf00_0000], [0x7e01, 0xfe2a]),
        ([0x7f80_2000, 0xff80_2000], [0, 0x8000]),
        ([0, 0x8000_0000], [0x7c01, 0xfc01]),
        ([0x7fc0_2000, 0xffc5_4000], [0x7e2a, 0xfe01]),
        ([0x7f80_2000, 0xff80_4000], [0xfc03, 0x7c02]),
        ([0, 0x8000_0000], [0xfc00, 0x7c00]),
    ]
    .into_iter()
    .enumerate()
    {
        add("nonfinite-diagnostic", format!("nonfinite-{i}"), s, d);
    }
    // Probe tiny addends at signed zero, with/without source half narrowing.
    // The original 95 pairs above remain unchanged and in their original order.
    for (i, bits) in [0x3300_0000, 0x3300_0001, 0x337f_ffff, 0x3380_0000]
        .into_iter().enumerate()
    {
        add("zero-underflow-order", format!("positive-tiny-negative-zero-{i}"),
            [bits, bits], [0x8000, 0x0000]);
    }
    // Cancellation distinguishes summing the actual float return from narrowing
    // that source to half before fixed-function addition. Keep old99 unchanged.
    for (i, (bits, destination)) in [
        (0x3f80_0001, 0xbc00), (0x3f7f_ffff, 0xbc00),
        (0x4000_0001, 0xc000), (0x3eff_ffff, 0xb800),
    ].into_iter().enumerate() {
        add("cancellation-order", format!("float-source-cancellation-{i}"),
            [bits, bits ^ 0x8000_0000], [destination, destination ^ 0x8000]);
    }
    assert_eq!(out.len(), 103);
    assert!(out.len() <= 128);
    out
}

fn specs() -> Vec<Spec> {
    let mut out = Vec::new();
    for precision in [32, 16] {
        out.push(Spec {
            name: format!("f{precision}-default"),
            precision,
            mode: "default",
            mask: 15,
            tag: 0,
            negative: false,
        });
        for mode in ["replace", "add"] {
            for (mask, name) in [(8, "r"), (4, "g"), (12, "rg"), (0, "none")] {
                out.push(Spec {
                    name: format!("f{precision}-{mode}-{name}"),
                    precision,
                    mode,
                    mask,
                    tag: 0,
                    negative: false,
                });
            }
        }
    }
    out.push(Spec {
        name: "f32-negative-skip-row".into(),
        precision: 32,
        mode: "replace",
        mask: 12,
        tag: 0,
        negative: true,
    });
    for (i, spec) in out.iter_mut().enumerate() {
        spec.tag = i as u32 + 1;
    }
    assert_eq!(out.len(), 19);
    out
}

fn word_offset(row: usize, x: usize, lane: usize) -> usize {
    GUARD + row * PITCH + x * 4 + lane * 2
}
fn read16(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([b[p], b[p + 1]])
}
fn read32(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn put16(b: &mut [u8], p: usize, x: u16) {
    b[p..p + 2].copy_from_slice(&x.to_le_bytes());
}

fn seed(rows: &[Row]) -> Vec<u8> {
    let mut b = vec![POISON; GUARD * 2 + (rows.len() + 2) * PITCH];
    let borders = [0x8000, 0x0001, 0x8001, 0x7c00, 0xfc00, 0x7e35, 0xfe17, 0x7c01, 0xfc02, 0x3555];
    for y in 0..rows.len() + 2 {
        for x in 0..WIDTH {
            for lane in 0..2 {
                let value = if x == 1 && y > 0 && y <= rows.len() {
                    rows[y - 1].destination[lane]
                } else {
                    borders[(y * 6 + x * 2 + lane) % borders.len()]
                };
                put16(&mut b, word_offset(y, x, lane), value);
            }
        }
    }
    b
}

fn source(rows: &[Row]) -> Vec<u8> {
    rows.iter().flat_map(|r| r.source.iter().flat_map(|v| v.to_le_bytes())).collect()
}

fn manifest(rows: &[Row]) -> String {
    let cases = rows.iter().enumerate().map(|(i, r)| format!(
        "{{\"id\":{i},\"kind\":\"{}\",\"name\":\"{}\",\"source_f32_hex\":[\"{:08x}\",\"{:08x}\"],\"destination_f16_hex\":[\"{:04x}\",\"{:04x}\"]}}",
        r.kind, r.name, r.source[0], r.source[1], r.destination[0], r.destination[1])).collect::<Vec<_>>().join(",\n");
    let captures = specs().iter().map(|s| format!(
        "{{\"name\":\"{}\",\"precision\":{},\"mode\":\"{}\",\"mask\":{},\"tag\":{},\"skip\":{},\"negative\":{}}}",
        s.name, s.precision, s.mode, s.mask, s.tag, if s.negative { SKIP as u32 } else { u32::MAX }, s.negative)).collect::<Vec<_>>().join(",\n");
    format!("{{\"schema\":1,\"count\":{},\"width\":3,\"height\":{},\"pitch\":256,\"guardBytes\":256,\"traceGuard\":64,\"cases\":[\n{cases}\n],\"captures\":[\n{captures}\n]}}\n", rows.len(), rows.len() + 2)
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    require(data.len() <= 1_048_576, "output exceeds per-file limit")?;
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .and_then(|mut f| f.write_all(data))
        .map_err(|e| format!("write: {}", e.kind()))
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("read metadata: {}", e.kind()))?;
    require(
        m.is_file() && !m.file_type().is_symlink() && m.len() <= 1_048_576,
        "input file bounds/type",
    )?;
    fs::read(path).map_err(|e| format!("read: {}", e.kind()))
}

fn generate(out: &Path) -> Result<(), String> {
    let r = rows();
    write_new(&out.join("source.bin"), &source(&r))?;
    write_new(&out.join("seed.bin"), &seed(&r))?;
    write_new(&out.join("cases.json"), manifest(&r).as_bytes())?;
    println!("synthetic_rows={} captures={} target={}x{}", r.len(), specs().len(), WIDTH, r.len() + 2);
    Ok(())
}

struct Capture {
    target: Vec<u8>,
    trace: Vec<u8>,
}

fn guards(c: &Capture, count: usize) -> Result<(), String> {
    let height = count + 2;
    require(c.target.len() == GUARD * 2 + height * PITCH, "target length")?;
    require(c.trace.len() == TRACE_GUARD * 2 + count * 16, "trace length")?;
    require(
        c.target[..GUARD].iter().chain(c.target[GUARD + height * PITCH..].iter()).all(|v| *v == POISON),
        "target guards",
    )?;
    for y in 0..height {
        require(
            c.target[GUARD + y * PITCH + WIDTH * 4..GUARD + (y + 1) * PITCH]
                .iter()
                .all(|v| *v == POISON),
            format!("row padding y={y}"),
        )?;
    }
    require(
        c.trace[..TRACE_GUARD]
            .iter()
            .chain(c.trace[TRACE_GUARD + count * 16..].iter())
            .all(|v| *v == POISON),
        "trace guards",
    )
}

fn shader_bits(c: &Capture, i: usize, lane: usize) -> u32 {
    read32(&c.trace, TRACE_GUARD + i * 16 + 8 + lane * 4)
}

// This is THE ordinary validator. It has no skip/negative exemption.
fn validate_capture(c: &Capture, s: &Spec, rows: &[Row], baseline: &[u8]) -> Result<(), String> {
    guards(c, rows.len())?;
    for (i, row) in rows.iter().enumerate() {
        let p = TRACE_GUARD + i * 16;
        require(
            read32(&c.trace, p) == MAGIC ^ s.tag && read32(&c.trace, p + 4) == i as u32,
            format!("trace-marker row={i}"),
        )?;
        for lane in 0..2 {
            let bits = shader_bits(c, i, lane);
            if s.precision == 32 && f32::from_bits(row.source[lane]).is_finite() {
                require(bits == row.source[lane], format!("finite float source row={i} lane={lane}"))?;
            } else if s.precision == 16 {
                require(bits <= 0xffff, "half trace width")?;
            }
            if row.kind == "anchor" {
                let sh = half(row.source[lane], true);
                if s.precision == 16 {
                    require(bits == u32::from(sh), "half exact source anchor")?;
                }
                if s.mask & [8, 4][lane] != 0 {
                    let value = if s.mode == "add" {
                        hfloat(sh) + hfloat(row.destination[lane])
                    } else {
                        hfloat(sh)
                    };
                    let expected = half(value.to_bits(), true);
                    // Only exactly representable finite anchors qualify. In particular,
                    // the large ADD anchor overflows half; its output stays diagnostic.
                    if value.is_finite() && hfloat(expected).to_bits() == value.to_bits() {
                        require(
                            read16(&c.target, word_offset(i + 1, 1, lane)) == expected,
                            format!("normal-anchor row={i} lane={lane}"),
                        )?;
                    }
                }
            }
        }
    }
    for y in 0..rows.len() + 2 {
        for x in 0..WIDTH {
            for (lane, bit) in [8, 4].into_iter().enumerate() {
                if x != 1 || y == 0 || y == rows.len() + 1 || s.mask & bit == 0 {
                    let p = word_offset(y, x, lane);
                    require(
                        c.target[p..p + 2] == baseline[p..p + 2],
                        format!("preservation y={y} x={x} lane={lane}"),
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn receipt_check(out: &Path, rows: &[Row], specs: &[Spec]) -> Result<(), String> {
    let text =
        String::from_utf8(read_bounded(&out.join("receipts.tsv"))?).map_err(|_| "receipt UTF-8")?;
    let lines: Vec<_> = text.lines().collect();
    require(lines.len() == specs.len() + 2 && lines[0] == RECEIPT_HEADER, "receipt coverage/header")?;
    for (i, line) in lines[1..].iter().enumerate() {
        let fields: Vec<_> = line.split('\t').collect();
        require(fields.len() == 18, "receipt fields")?;
        let number = |n: usize| fields[n].parse::<u32>().map_err(|_| "receipt integer".to_string());
        require(
            number(0)? == i as u32 && fields[1] == if i == 0 { "baseline" } else { &specs[i - 1].name },
            "receipt identity/order",
        )?;
        require(number(2)? == 4 && number(3)? == 0, "receipt command not completed/error")?;
        // Public MTLCommandBufferStatusCompleted is 4, not a host-generated success bit.
        let start: f64 = fields[4].parse().map_err(|_| "GPU start time")?;
        let end: f64 = fields[5].parse().map_err(|_| "GPU end time")?;
        require(
            start.is_finite() && end.is_finite() && start >= 0.0 && end >= start,
            "GPU timing range",
        )?;
        require(
            number(6)? == 65 && number(7)? == WIDTH as u32 && number(8)? == (rows.len() + 2) as u32,
            "receipt actual texture",
        )?;
        let expected = if i == 0 {
            [0; 9]
        } else {
            let s = &specs[i - 1];
            if s.mode == "default" {
                [15, 0, 0, 0, 1, 0, 1, 0, s.precision]
            } else {
                [s.mask, u32::from(s.mode == "add"), 0, 0, 1, 1, 1, 1, s.precision]
            }
        };
        for (n, want) in expected.into_iter().enumerate() {
            require(number(n + 9)? == want, "receipt pipeline state")?;
        }
    }
    Ok(())
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
                "+infinity"
            } else {
                "-infinity"
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

// Diagnostic models ONLY: binary32 sum, optionally narrowing the traced source
// to half first (RNE or RTZ), followed by independent RNE or RTZ half output.
// Nonfinite operands are observed, never arithmetically qualified.
fn model(source: f32, destination: u16, id: usize) -> u16 {
    assert!(id < 6);
    let s = match id / 2 {
        0 => source,
        1 => hfloat(half(source.to_bits(), true)),
        _ => hfloat(half(source.to_bits(), false)),
    };
    half((s + hfloat(destination)).to_bits(), id & 1 == 0)
}

fn validate(out: &Path) -> Result<(), String> {
    let rows = rows();
    let specs = specs();
    require(read_bounded(&out.join("source.bin"))? == source(&rows), "source input changed")?;
    require(read_bounded(&out.join("seed.bin"))? == seed(&rows), "seed input changed")?;
    require(read_bounded(&out.join("cases.json"))? == manifest(&rows).as_bytes(), "manifest changed")?;
    receipt_check(out, &rows, &specs)?;
    let baseline = read_bounded(&out.join("baseline-target.bin"))?;
    require(baseline == seed(&rows), "GPU baseline is not a bit-exact upload/readback")?;
    let mut captures = Vec::new();
    for s in &specs {
        captures.push(Capture {
            target: read_bounded(&out.join(format!("{}-target.bin", s.name)))?,
            trace: read_bounded(&out.join(format!("{}-trace.bin", s.name)))?,
        });
    }
    for (s, c) in specs.iter().zip(&captures).filter(|(s, _)| !s.negative) {
        validate_capture(c, s, &rows, &baseline).map_err(|e| format!("{}: {e}", s.name))?;
    }
    // A disabled-blend descriptor must agree with the independent GPU default
    // replacement on finite returned values. This is GPU-vs-GPU, NOT a chosen
    // CPU rounding rule. Nonfinite returns remain fully retained diagnostics.
    let mut replace_control_lanes = 0;
    for (s, c) in specs.iter().zip(&captures).filter(|(s, _)| s.mode == "replace" && !s.negative) {
        let control = &captures[if s.precision == 32 { 0 } else { 9 }];
        for i in 0..rows.len() {
            for (lane, bit) in [8, 4].into_iter().enumerate() {
                let bits = shader_bits(c, i, lane);
                let reference = shader_bits(control, i, lane);
                let finite = |v| {
                    if s.precision == 32 {
                        f32::from_bits(v).is_finite()
                    } else {
                        hfloat(v as u16).is_finite()
                    }
                };
                if s.mask & bit != 0 && (finite(bits) || finite(reference)) {
                    let p = word_offset(i + 1, 1, lane);
                    require(
                        bits == reference && c.target[p..p + 2] == control.target[p..p + 2],
                        format!("{}: independent finite default-replace row={i} lane={lane}", s.name),
                    )?;
                    replace_control_lanes += 1;
                }
            }
        }
    }
    let n = &captures[18];
    let rejection = match validate_capture(n, &specs[18], &rows, &baseline) {
        Err(reason) => reason,
        Ok(()) => return Err("GPU negative unexpectedly passed the ordinary validator".into()),
    };
    require(
        rejection == format!("trace-marker row={SKIP}"),
        "negative did not isolate skipped GPU row",
    )?;
    guards(n, rows.len())?;
    let positive = &captures[3]; // f32-replace-rg
    for i in 0..rows.len() {
        let t = TRACE_GUARD + i * 16;
        if i == SKIP {
            require(
                n.trace[t..t + 16].iter().all(|v| *v == POISON),
                "negative skipped trace was written",
            )?;
            let p = word_offset(i + 1, 1, 0);
            require(
                n.target[p..p + 4] == baseline[p..p + 4]
                    && n.target[p..p + 4] != positive.target[p..p + 4],
                "negative actual target row was not skipped",
            )?;
        } else {
            require(
                read32(&n.trace, t) == MAGIC ^ specs[18].tag
                    && read32(&n.trace, t + 4) == i as u32
                    && n.trace[t + 8..t + 16] == positive.trace[t + 8..t + 16],
                "negative changed another shader trace row",
            )?;
        }
    }
    // Validate all non-skipped bytes with an ordinary positive spec by inspection;
    // never repair the captured negative data or pretend a CPU fixture is evidence.
    for y in 0..rows.len() + 2 {
        for x in 0..WIDTH {
            for lane in 0..2 {
                let p = word_offset(y, x, lane);
                if x != 1 || y == 0 || y == rows.len() + 1 {
                    require(n.target[p..p + 2] == baseline[p..p + 2], "negative target border")?;
                } else if y - 1 != SKIP && rows[y - 1].kind == "anchor" {
                    require(n.target[p..p + 2] == positive.target[p..p + 2], "negative other anchor")?;
                }
            }
        }
    }

    let mut table = String::from("capture\trow\tkind\tcase\tlane\twrite_enabled\tsource_f32\tshader_return_bits\tdestination_f16\tactual_f16\tactual_class\tdefault_replace_f16\texplicit_replace_rg_f16\tadd_rg_f16\tadd_models_0_to_5_diagnostic\n");
    let mut counts = [[0usize; 6]; 2];
    let mut totals = [0usize; 2];
    let mut replace_differences = [0usize; 2];
    for (index, (s, c)) in specs.iter().zip(&captures).enumerate() {
        if s.negative {
            continue;
        }
        let pi = usize::from(s.precision == 16);
        let base = pi * 9;
        for (i, row) in rows.iter().enumerate() {
            for lane in 0..2 {
                let p = word_offset(i + 1, 1, lane);
                let bits = shader_bits(c, i, lane);
                let src = if s.precision == 32 { f32::from_bits(bits) } else { hfloat(bits as u16) };
                let dst = read16(&baseline, p);
                let actual = read16(&c.target, p);
                let default = read16(&captures[base].target, p);
                let replace = read16(&captures[base + 3].target, p);
                let add = read16(&captures[base + 7].target, p);
                let predictions = if src.is_finite() && hfloat(dst).is_finite() {
                    (0..6).map(|id| format!("{:04x}", model(src, dst, id))).collect::<Vec<_>>().join(",")
                } else {
                    "nonfinite-observe-only".into()
                };
                if index == base + 7 && src.is_finite() && hfloat(dst).is_finite() {
                    totals[pi] += 1;
                    for id in 0..6 {
                        counts[pi][id] += usize::from(model(src, dst, id) != actual);
                    }
                }
                if index == base + 3 {
                    replace_differences[pi] += usize::from(default != actual);
                }
                table.push_str(&format!("{}\t{i}\t{}\t{}\t{}\t{}\t{:08x}\t{bits:08x}\t{dst:04x}\t{actual:04x}\t{}\t{default:04x}\t{replace:04x}\t{add:04x}\t{predictions}\n",
                    s.name, row.kind, row.name, ["R", "G"][lane], u32::from(s.mask & [8, 4][lane] != 0), row.source[lane], classify(actual)));
            }
        }
    }
    write_new(&out.join("observations.tsv"), table.as_bytes())?;
    let summary = format!(concat!(
        "{{\n\"schema\":1,\"status\":\"OBSERVATIONS_COMPLETE\",\"rows\":{},\"positive_captures\":18,\"completed_commands\":20,\n",
        "\"ordinary_validator_negative_rejected\":true,\"negative_skipped_row\":2,\"guards_checked\":true,\"all_masked_words_preserved\":true,\n",
        "\"independent_default_replace_anchors_checked\":true,\"finite_gpu_vs_gpu_replace_lanes_checked\":{},\"mask_zero_includes_nonfinite_words\":true,\n",
        "\"finite_model_lanes_f32_f16\":{:?},\"model_mismatches_f32\":{:?},\"model_mismatches_f16\":{:?},\n",
        "\"replace_vs_default_all_lane_differences_f32_f16\":{:?},\n",
        "\"model_order\":[\"binary32-sum-then-RNE\",\"binary32-sum-then-RTZ\",\"source-half-RNE-binary32-sum-then-RNE\",\"source-half-RNE-binary32-sum-then-RTZ\",\"source-half-RTZ-binary32-sum-then-RNE\",\"source-half-RTZ-binary32-sum-then-RTZ\"],\n",
        "\"model_status\":\"diagnostic only; zero mismatches is consistency, not uniqueness or a universal rule\",\n",
        "\"nonfinite_status\":\"all output bits retained; no NaN/Infinity arithmetic qualification\",\n",
        "\"factor_ignorance_measured\":false,\"alpha_output_measured\":false,\"cpu_generated_gpu_outputs\":false\n}}\n"), rows.len(), replace_control_lanes, totals, counts[0], counts[1], replace_differences);
    write_new(&out.join("validation.json"), summary.as_bytes())?;
    println!("OBSERVATIONS_COMPLETE: 18 positive captures; GPU skipped-row negative rejected; exact bits retained; models are diagnostic");
    Ok(())
}

fn main() {
    let args: Vec<_> = env::args_os().collect();
    let result = if args.len() == 3 {
        let out = Path::new(&args[2]);
        if args[1] == "generate" {
            generate(out)
        } else if args[1] == "validate" {
            validate(out)
        } else {
            Err("expected generate|validate OUTPUT_DIRECTORY".into())
        }
    } else {
        Err("expected generate|validate OUTPUT_DIRECTORY".into())
    };
    if let Err(e) = result {
        eprintln!("NOT_QUALIFIED: {e}");
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // CPU fixtures exist ONLY in this test build, are never saved, and are not
    // observations. They test that the ordinary validator rejects corruption.
    fn fixture(s: &Spec, rows: &[Row]) -> Capture {
        let mut c =
            Capture { target: seed(rows), trace: vec![POISON; TRACE_GUARD * 2 + rows.len() * 16] };
        for (i, r) in rows.iter().enumerate() {
            let p = TRACE_GUARD + i * 16;
            let values = [
                MAGIC ^ s.tag,
                i as u32,
                if s.precision == 32 { r.source[0] } else { u32::from(half(r.source[0], true)) },
                if s.precision == 32 { r.source[1] } else { u32::from(half(r.source[1], true)) },
            ];
            for (k, v) in values.into_iter().enumerate() {
                c.trace[p + k * 4..p + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            if r.kind == "anchor" {
                for lane in 0..2 {
                    if s.mask & [8, 4][lane] != 0 {
                        let v = if s.mode == "add" {
                            f32::from_bits(r.source[lane]) + hfloat(r.destination[lane])
                        } else {
                            f32::from_bits(r.source[lane])
                        };
                        put16(&mut c.target, word_offset(i + 1, 1, lane), half(v.to_bits(), true));
                    }
                }
            }
        }
        c
    }

    #[test]
    fn finite_half_roundtrip_exhaustive() {
        for h in 0..=u16::MAX {
            if h & 0x7c00 != 0x7c00 {
                assert_eq!(half(hfloat(h).to_bits(), true), h);
                assert_eq!(half(hfloat(h).to_bits(), false), h);
            }
        }
    }

    #[test]
    fn conversion_hypotheses_are_distinct_at_ties() {
        assert_eq!(half(1.00048828125f32.to_bits(), true), 0x3c00);
        assert_eq!(half(1.00146484375f32.to_bits(), true), 0x3c02);
        assert_eq!(half(1.00146484375f32.to_bits(), false), 0x3c01);
        assert_eq!(half(65520.0f32.to_bits(), true), 0x7c00);
        assert_eq!(half(65520.0f32.to_bits(), false), 0x7bff);
        assert_eq!(half(0x3300_0000, true), 0);
        assert_eq!(half(0x3300_0001, true), 1);
        assert_eq!(half(0x8000_0001, true), 0x8000);
    }

    #[test]
    fn add_models_are_sums_with_independent_source_and_output_rounding() {
        for id in 0..6 {
            assert_eq!(model(0.75, 0x3400, id), 0x3c00); // 0.75 + 0.25 = 1
            assert_eq!(model(-2.0, 0xc400, id), 0xc600); // -2 + -4 = -6
            assert_eq!(model(-0.0, 0x8000, id), 0x8000);
        }
        let source = 1.00146484375f32;
        assert_eq!((0..6).map(|id| model(source, 0xbc00, id)).collect::<Vec<_>>(),
            [0x1600, 0x1600, 0x1800, 0x1800, 0x1400, 0x1400]);
        assert_eq!((0..6).map(|id| model(source, 0, id)).collect::<Vec<_>>(),
            [0x3c02, 0x3c01, 0x3c02, 0x3c02, 0x3c01, 0x3c01]);
    }

    #[test]
    fn ordinary_validator_rejects_non_add_anchor_outputs() {
        let r = rows();
        for spec in [5, 6, 7, 14, 15, 16] {
            let s = &specs()[spec];
            for (lane, bit) in [8, 4].into_iter().enumerate() {
                if s.mask & bit != 0 {
                    let mut c = fixture(s, &r);
                    // Row 0 replacement and extremum selection both differ from ADD.
                    put16(&mut c.target, word_offset(1, 1, lane), half(r[0].source[lane], true));
                    assert!(validate_capture(&c, s, &r, &seed(&r)).unwrap_err().starts_with("normal-anchor"));
                }
            }
        }
    }

    #[test]
    fn add_overflow_is_diagnostic_but_replace_anchor_remains_exact() {
        let r = rows();
        for spec in [7, 16] {
            let s = &specs()[spec];
            for output in [[0x7bff, 0xfbff], [0x7c00, 0xfc00]] {
                let mut c = fixture(s, &r);
                for (lane, h) in output.into_iter().enumerate() {
                    put16(&mut c.target, word_offset(7, 1, lane), h);
                }
                validate_capture(&c, s, &r, &seed(&r)).unwrap();
            }
        }
        for spec in [3, 12] {
            let s = &specs()[spec];
            let mut c = fixture(s, &r);
            put16(&mut c.target, word_offset(7, 1, 0), 0x7c00);
            assert!(validate_capture(&c, s, &r, &seed(&r)).unwrap_err().starts_with("normal-anchor"));
        }
    }

    #[test]
    fn bounded_deterministic_inputs_and_controls() {
        let r = rows();
        assert_eq!(manifest(&r), manifest(&rows()));
        assert_eq!(source(&r).len(), r.len() * 8);
        assert!(seed(&r).len() < 34_000);
        let s = specs();
        assert_eq!(s[3].name, "f32-replace-rg");
        assert_eq!(s[7].name, "f32-add-rg");
        assert_eq!(s[12].name, "f16-replace-rg");
        assert_eq!(s[16].name, "f16-add-rg");
        assert!(r.iter().any(|v| v.destination.contains(&0x7c01)));
        for spec in s {
            validate_capture(&fixture(&spec, &r), &spec, &r, &seed(&r)).unwrap();
        }
    }

    #[test]
    fn ordinary_validator_rejects_missing_gpu_row() {
        let r = rows();
        let s = &specs()[18];
        let mut c = fixture(s, &r);
        c.trace[TRACE_GUARD + SKIP * 16..TRACE_GUARD + (SKIP + 1) * 16].fill(POISON);
        let p = word_offset(SKIP + 1, 1, 0);
        c.target[p..p + 4].copy_from_slice(&seed(&r)[p..p + 4]);
        assert_eq!(validate_capture(&c, s, &r, &seed(&r)).unwrap_err(), "trace-marker row=2");
    }

    #[test]
    fn ordinary_validator_rejects_padding_guard_mask_and_anchor_corruption() {
        let r = rows();
        for (spec, offset) in [
            (3, 0),
            (3, GUARD + 12),
            (3, word_offset(1, 1, 0)),
            (1, word_offset(1, 1, 1)),
            (4, word_offset(1, 1, 0)),
            (7, 0),
            (7, GUARD + 12),
            (7, word_offset(1, 1, 0)),
            (6, word_offset(1, 1, 0)),
            (5, word_offset(1, 1, 1)),
            (8, word_offset(1, 1, 0)),
        ] {
            let s = &specs()[spec];
            let mut c = fixture(s, &r);
            c.target[offset] ^= 1;
            assert!(validate_capture(&c, s, &r, &seed(&r)).is_err());
        }
    }

    #[test]
    fn zero_mask_checks_nonfinite_payload_bits() {
        let r = rows();
        for spec in [4, 8, 13, 17] {
            let s = &specs()[spec];
            let mut c = fixture(s, &r);
            let row = r.iter().position(|v| v.destination.contains(&0x7c01)).unwrap();
            let p = word_offset(row + 1, 1, 0);
            put16(&mut c.target, p, read16(&seed(&r), p) | 0x0200);
            assert!(validate_capture(&c, s, &r, &seed(&r)).unwrap_err().starts_with("preservation"));
        }
    }

    #[test]
    fn ordinary_validator_rejects_trace_guard_marker_and_return_corruption() {
        let r = rows();
        for spec in [7, 16] {
            let s = &specs()[spec];
            for offset in [0, TRACE_GUARD + r.len() * 16, TRACE_GUARD, TRACE_GUARD + 4,
                TRACE_GUARD + 8, TRACE_GUARD + 12] {
                let mut c = fixture(s, &r);
                c.trace[offset] ^= 1;
                assert!(validate_capture(&c, s, &r, &seed(&r)).is_err());
            }
        }
    }
}
