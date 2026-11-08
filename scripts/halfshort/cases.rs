// Standalone synthetic inputs, AIR-call check, and raw GPU validator.
// No native interop, dependencies, installs, CPU result fallback, or normalization.
#![forbid(unsafe_code)]
use std::{env, fs, io::Write, path::Path};

const HALF_COUNT: usize = 65536;
const GUARD: usize = 64;
const STRIDE: usize = 12;
const HALF_SKIP: usize = 0x3c00; // +1: known non-poison output in both controls.
const INT_SKIP: usize = 1; // +1.
const KERNELS: [&str; 3] = ["half-short", "half-int", "int-half"];
const MODES: [&str; 2] = ["nofast", "fast"];

fn need(ok: bool, why: impl Into<String>) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(why.into())
    }
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("read metadata: {}", e.kind()))?;
    need(m.is_file() && !m.file_type().is_symlink() && m.len() <= 1_048_576, "file size/type")?;
    fs::read(path).map_err(|e| format!("read: {}", e.kind()))
}
fn save(path: &Path, bytes: &[u8]) -> Result<(), String> {
    need(bytes.len() <= 1_048_576, "file byte cap")?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("write: {}", e.kind()))
}
fn text(path: &Path) -> Result<String, String> {
    String::from_utf8(read(path)?).map_err(|_| "UTF-8 required".into())
}
fn quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}
fn u16at(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([b[p], b[p + 1]])
}
fn u32at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn half_inputs() -> Vec<u8> {
    (0..=u16::MAX).flat_map(u16::to_le_bytes).collect()
}
fn ints() -> Vec<i32> {
    let mut v = vec![0];
    for n in [1, 2048, 2049, 2050, 2051, 2052, 65503, 65504, 65505, 65519, 65520, 65521, 65535, 65536] {
        v.extend([n, -n]);
    }
    v.extend([i32::MIN, i32::MAX]);
    assert_eq!(v.len(), 31);
    v
}
fn int_inputs() -> Vec<u8> {
    ints().iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn manifest() -> String {
    let values = ints()
        .iter()
        .enumerate()
        .map(|(i, x)| format!("{{\"row\":{i},\"value\":{x},\"bits\":\"{:08x}\"}}", *x as u32))
        .collect::<Vec<_>>()
        .join(",");
    format!(concat!("{{\"schema\":1,\"halfCount\":65536,\"intCount\":31,\"guardBytes\":64,\"stride\":12,",
        "\"halfSkip\":15360,\"intSkip\":1,\"halfOrder\":\"all u16 bit patterns in ascending order\",",
        "\"rowLayout\":\"value at 0; echo at 4; u32 marker at 8; untouched u16 at 2 (short/half) and 6 (short/int)\",",
        "\"values\":[{}]}}\n"), values)
}
fn generate(out: &Path) -> Result<(), String> {
    save(&out.join("half-input.bin"), &half_inputs())?;
    save(&out.join("int-input.bin"), &int_inputs())?;
    save(&out.join("cases.json"), manifest().as_bytes())?;
    println!("synthetic_half_patterns=65536 signed_int_inputs=31 modes=2 captures=4");
    Ok(())
}

fn body<'a>(ir: &'a str, kernel: &str) -> Result<&'a str, String> {
    let mut offset = 0;
    for line in ir.split_inclusive('\n') {
        if line.trim_start().starts_with("define ")
            && (line.contains(&format!("@{kernel}(")) || line.contains(&format!("@\"{kernel}\"(")))
        {
            let remainder = &ir[offset..];
            let end = remainder.find("\n}").ok_or("unterminated kernel body")? + 2;
            return Ok(&remainder[..end]);
        }
        offset += line.len();
    }
    Err(format!("missing definition: {kernel}"))
}

fn prove_call(
    ir: &str,
    kernel: &str,
    intrinsic: &str,
    result_ty: &str,
    argument_ty: &str,
) -> Result<String, String> {
    let b = body(ir, kernel)?;
    need(
        !b.contains("fpext") && !b.contains("fptrunc") && !b.contains("air.convert.f.f32"),
        "wider float route in kernel",
    )?;
    if kernel == "convert_half_short" {
        need(
            !b.contains("air.convert.s.i32") && !b.contains("air.convert.s.i64"),
            "wider integer route in narrow kernel",
        )?;
    }
    let needle = format!("@{intrinsic}(");
    for line in b.lines().map(str::trim).filter(|l| !l.starts_with(';')) {
        if !line.contains("call ") || !line.contains(&needle) {
            continue;
        }
        let (result, call) = line.split_once(" = ").ok_or("conversion result is not named")?;
        let (before, after) = call.split_once(&needle).ok_or("call target")?;
        need(
            result.starts_with('%')
                && before.split_whitespace().last() == Some(result_ty)
                && after.split_whitespace().next() == Some(argument_ty),
            "conversion call signature",
        )?;
        let mut stored = b.contains(&format!("store {result_ty} {result},"));
        if result_ty == "half" {
            for cast in b.lines().map(str::trim) {
                if let Some((alias, rhs)) = cast.split_once(" = ") {
                    if rhs == format!("bitcast half {result} to i16")
                        || rhs.starts_with(&format!("bitcast half {result} to i16,"))
                    {
                        stored |= b.contains(&format!("store i16 {alias},"));
                    }
                }
            }
        }
        need(
            stored,
            "conversion result has no direct output store (declaration/dead call is not proof)",
        )?;
        return Ok(line.to_string());
    }
    Err(format!("missing actual call: {intrinsic}"))
}

fn verify_air(path: &Path, output: &Path) -> Result<(), String> {
    let ir = text(path)?;
    let mut proofs = Vec::new();
    let mut errors = Vec::new();
    for (kernel, intrinsic, rt, at) in [
        ("convert_half_short", "air.convert.s.i16.f.f16", "i16", "half"),
        ("convert_half_int", "air.convert.s.i32.f.f16", "i32", "half"),
        ("convert_int_half", "air.convert.f.f16.s.i32", "half", "i32"),
    ] {
        match prove_call(&ir, kernel, intrinsic, rt, at) {
            Ok(call) => proofs.push(format!(
                "{{\"kernel\":{},\"intrinsic\":{},\"call\":{}}}",
                quote(kernel),
                quote(intrinsic),
                quote(&call)
            )),
            Err(e) => errors.push(quote(&format!("{kernel}: {e}"))),
        }
    }
    let good = errors.is_empty();
    let json = format!("{{\"schema\":1,\"qualified\":{good},\"evidence_kind\":\"typed live calls in supplied IR; exact AIR/library provenance and GPU execution require matching compiler/runtime receipts\",\"calls\":[{}],\"errors\":[{}]}}\n", proofs.join(","), errors.join(","));
    save(output, json.as_bytes())?;
    need(good, "UNQUALIFIED_AIR_SIGNATURE; native raw observations may still be retained")
}

fn hvalue(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let e = (h >> 10) & 31;
    let m = u32::from(h & 1023);
    let bits = match e {
        0 => ((m as f32) * f32::from_bits(0x3380_0000)).to_bits(),
        31 => 0x7f80_0000 | (m << 13),
        _ => ((u32::from(e) + 112) << 23) | (m << 13),
    };
    f32::from_bits(sign | bits)
}
fn category(h: u16) -> usize {
    let v = hvalue(h);
    if v.is_nan() {
        4
    } else if v == f32::INFINITY {
        2
    } else if v == f32::NEG_INFINITY {
        3
    } else if (-32768.0..32768.0).contains(&v) {
        0
    } else {
        1
    }
}
fn row_count(kernel: usize) -> usize {
    if kernel == 2 {
        ints().len()
    } else {
        HALF_COUNT
    }
}
fn tag(capture: usize, kernel: usize) -> u32 {
    (capture * 3 + kernel + 1) as u32
}
fn marker(tag: u32, row: usize) -> u32 {
    0x5a00_0000 | (tag << 16) | row as u32
}
fn stem(capture: usize) -> String {
    format!("{}-{}", MODES[capture / 2], if capture % 2 == 0 { "positive" } else { "negative" })
}

fn guards(raw: &[u8], kernel: usize) -> Result<(), String> {
    let n = row_count(kernel);
    need(raw.len() == GUARD * 2 + n * STRIDE, "result length")?;
    need(
        raw[..GUARD].iter().chain(raw[GUARD + n * STRIDE..].iter()).all(|b| *b == 0xcd),
        "result end guards",
    )?;
    for i in 0..n {
        let p = GUARD + i * STRIDE;
        if kernel != 1 {
            need(u16at(raw, p + 2) == 0xcdcd, format!("row guard at 2: {i}"))?;
        }
        if kernel != 2 {
            need(u16at(raw, p + 6) == 0xcdcd, format!("row guard at 6: {i}"))?;
        }
    }
    Ok(())
}

// Ordinary validator has NO negative/skip exemption. Nonfinite/out-of-range
// numeric results and value==poison are not blanket-rejected as conversion errors.
fn ordinary(raw: &[u8], capture: usize, kernel: usize) -> Result<(), String> {
    guards(raw, kernel)?;
    for i in 0..row_count(kernel) {
        let p = GUARD + i * STRIDE;
        need(u32at(raw, p + 8) == marker(tag(capture, kernel), i), format!("missing marker row={i}"))?;
        if kernel == 2 {
            need(u32at(raw, p + 4) == ints()[i] as u32, "integer input echo changed")?;
        }
    }
    // Only exact, representable 0/+1/-1 anchors are arithmetic hard gates.
    for (i, value) in if kernel == 2 {
        vec![(0, 0), (1, 0x3c00), (2, 0xbc00)]
    } else if kernel == 0 {
        vec![(0, 0), (0x8000, 0), (0x3c00, 1), (0xbc00, 0xffff)]
    } else {
        vec![(0, 0), (0x8000, 0), (0x3c00, 1), (0xbc00, 0xffff_ffff)]
    } {
        let p = GUARD + i * STRIDE;
        let got = if kernel == 1 { u32at(raw, p) } else { u32::from(u16at(raw, p)) };
        need(got == value, format!("exact anchor kernel={kernel} row={i}"))?;
    }
    Ok(())
}

fn validate(out: &Path) -> Result<(), String> {
    need(read(&out.join("half-input.bin"))? == half_inputs(), "exhaustive half inputs changed")?;
    need(read(&out.join("int-input.bin"))? == int_inputs(), "integer inputs changed")?;
    need(read(&out.join("cases.json"))? == manifest().as_bytes(), "manifest changed")?;
    let receipt = text(&out.join("receipts.tsv"))?;
    let lines: Vec<_> = receipt.lines().collect();
    need(
        lines.len() == 5
            && lines[0] == "capture\tmode\tnegative\tstatus\terror\tgpu_start\tgpu_end\tlibrary_sha256",
        "receipt coverage/header",
    )?;
    let mut captures = Vec::new();
    for c in 0..4 {
        let fields: Vec<_> = lines[c + 1].split('\t').collect();
        need(
            fields.len() == 8
                && fields[0] == stem(c)
                && fields[1] == MODES[c / 2]
                && fields[2] == if c % 2 == 0 { "0" } else { "1" },
            "receipt identity",
        )?;
        need(
            fields[3] == "4"
                && fields[4] == "0"
                && fields[7].len() == 64
                && fields[7].bytes().all(|b| b.is_ascii_hexdigit()),
            "real command completion/library binding",
        )?;
        let start: f64 = fields[5].parse().map_err(|_| "GPU start")?;
        let end: f64 = fields[6].parse().map_err(|_| "GPU end")?;
        need(start.is_finite() && end.is_finite() && start >= 0.0 && end >= start, "GPU timing")?;
        let mut kernel_data = Vec::new();
        for k in 0..3 {
            let raw = read(&out.join(format!("{}-{}.bin", stem(c), KERNELS[k])))?;
            if c % 2 == 0 {
                ordinary(&raw, c, k)?;
            } else {
                let skip = if k == 2 { INT_SKIP } else { HALF_SKIP };
                need(
                    ordinary(&raw, c, k).err() == Some(format!("missing marker row={skip}")),
                    "real GPU negative was not rejected by ordinary validator",
                )?;
                guards(&raw, k)?;
                for i in 0..row_count(k) {
                    let p = GUARD + i * STRIDE;
                    if i == skip {
                        need(
                            raw[p..p + STRIDE].iter().all(|b| *b == 0xcd),
                            "negative skipped row not wholly untouched",
                        )?;
                    } else {
                        need(
                            u32at(&raw, p + 8) == marker(tag(c, k), i),
                            "negative affected another row",
                        )?;
                    }
                }
                let positive: &Vec<Vec<u8>> = &captures[c - 1];
                let p = GUARD + skip * STRIDE;
                need(
                    raw[p..p + 2] != positive[k][p..p + 2],
                    "negative output not distinct from positive anchor",
                )?;
            }
            kernel_data.push(raw);
        }
        captures.push(kernel_data);
    }
    let mut half_table = String::from(
        "mode\tinput_half\tshort_raw_u16\tshort_echo_half\twide_raw_u32\twide_echo_half\tdomain\n",
    );
    let mut int_table = String::from("mode\trow\tinput_i32\tinput_bits\techo_bits\tactual_half_bits\n");
    let mut summaries = Vec::new();
    let labels = [
        "finite-in-short-range",
        "finite-out-of-short-range",
        "positive-infinity",
        "negative-infinity",
        "NaN",
    ];
    let selected = [
        0u16, 0x8000, 1, 0x8001, 0x03ff, 0x83ff, 0x0400, 0x8400, 0x3800, 0xb800, 0x3bff, 0xbbff, 0x3c00,
        0xbc00, 0x3e00, 0xbe00, 0x77ff, 0xf7ff, 0x7800, 0xf800, 0x7801, 0xf801, 0x7bff, 0xfbff, 0x7c00,
        0xfc00, 0x7c01, 0xfc01, 0x7e00, 0xfe00, 0x7fff, 0xffff,
    ];
    for (mode, c) in MODES.iter().zip([0usize, 2]) {
        let data = &captures[c];
        let mut counts = [0usize; 5];
        let mut low16_equal = [0usize; 5];
        let mut echo_differences = [0usize; 5];
        let mut finite_wide_truncation_mismatches = 0;
        let mut inrange_short_truncation_mismatches = 0;
        for i in 0..HALF_COUNT {
            let p = GUARD + i * STRIDE;
            let domain = category(i as u16);
            counts[domain] += 1;
            let narrow = u16at(&data[0], p);
            let wide = u32at(&data[1], p);
            low16_equal[domain] += usize::from(narrow == wide as u16);
            echo_differences[domain] +=
                usize::from(u16at(&data[0], p + 4) != i as u16 || u16at(&data[1], p + 4) != i as u16);
            // Explicitly diagnostic hypotheses. Never cast a nonfinite float
            // on the CPU to invent a native integer result.
            if domain < 2 {
                let trunc = hvalue(i as u16) as i32; // every finite half fits i32
                finite_wide_truncation_mismatches += usize::from(wide != trunc as u32);
                if domain == 0 {
                    inrange_short_truncation_mismatches += usize::from(narrow != trunc as u16);
                }
            }
        }
        for h in selected {
            let p = GUARD + h as usize * STRIDE;
            half_table.push_str(&format!(
                "{mode}\t{h:04x}\t{:04x}\t{:04x}\t{:08x}\t{:04x}\t{}\n",
                u16at(&data[0], p),
                u16at(&data[0], p + 4),
                u32at(&data[1], p),
                u16at(&data[1], p + 4),
                labels[category(h)]
            ));
        }
        for (i, x) in ints().into_iter().enumerate() {
            let p = GUARD + i * STRIDE;
            int_table.push_str(&format!(
                "{mode}\t{i}\t{x}\t{:08x}\t{:08x}\t{:04x}\n",
                x as u32,
                u32at(&data[2], p + 4),
                u16at(&data[2], p)
            ));
        }
        summaries.push(format!("{{\"mode\":{mode},\"domain_counts\":{counts:?},\"narrow_equals_low16_of_observed_wide\":{low16_equal:?},\"input_echo_differences\":{echo_differences:?},\"finite_wide_truncation_hypothesis_mismatches\":{finite_wide_truncation_mismatches},\"inrange_short_truncation_hypothesis_mismatches\":{inrange_short_truncation_mismatches}}}", mode=quote(mode)));
    }
    save(&out.join("selected-half.tsv"), half_table.as_bytes())?;
    save(&out.join("int-results.tsv"), int_table.as_bytes())?;
    let json = format!(concat!("{{\"schema\":1,\"status\":\"RAW_OBSERVATIONS_VALIDATED_PENDING_AIR_GATE\",\"half_patterns\":65536,\"int_inputs\":31,",
        "\"completed_commands\":4,\"positive_kernel_captures\":6,\"ordinary_negative_rejections\":6,\"guards_checked\":true,",
        "\"domain_order\":[\"finite-in-short-range\",\"finite-out-of-short-range\",\"positive-infinity\",\"negative-infinity\",\"NaN\"],",
        "\"numeric_policy\":\"out-of-range/nonfinite words and all rounding hypotheses are observations, not blanket defined MSL contracts; only exact 0/+1/-1 anchors gate arithmetic\",",
        "\"modes\":[{}]}}\n"), summaries.join(","));
    save(&out.join("validation.json"), json.as_bytes())?;
    println!("raw_observations_validated: all patterns retained; six real GPU negative kernel captures rejected; AIR qualification is a separate gate");
    Ok(())
}

fn main() {
    let args: Vec<_> = env::args_os().collect();
    let result = if args.len() == 3 && args[1] == "generate" {
        generate(Path::new(&args[2]))
    } else if args.len() == 3 && args[1] == "validate" {
        validate(Path::new(&args[2]))
    } else if args.len() == 4 && args[1] == "verify-air" {
        verify_air(Path::new(&args[2]), Path::new(&args[3]))
    } else {
        Err("expected generate DIR | validate DIR | verify-air IR OUTPUT_JSON".into())
    };
    if let Err(e) = result {
        eprintln!("UNQUALIFIED: {e}");
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_half_patterns_once_and_int_edges() {
        let b = half_inputs();
        assert_eq!(b.len(), 131072);
        let mut counts = [0usize; 5];
        for i in 0..HALF_COUNT {
            assert_eq!(u16at(&b, i * 2), i as u16);
            counts[category(i as u16)] += 1;
        }
        assert_eq!(counts, [61441, 2047, 1, 1, 2046]);
        let values = ints();
        assert_eq!(values.len(), 31);
        assert_eq!(values[INT_SKIP], 1);
        for x in [2049, 2051, 65504, 65519, 65520, 65535, 65536] {
            assert!(values.contains(&x) && values.contains(&-x));
        }
        assert!(values.contains(&i32::MIN) && values.contains(&i32::MAX));
        assert_eq!(manifest(), manifest());
        assert!(GUARD * 2 + HALF_COUNT * STRIDE < 1_048_576);
        assert!(
            4 * (2 * (GUARD * 2 + HALF_COUNT * STRIDE) + GUARD * 2 + values.len() * STRIDE)
                < 7 * 1024 * 1024
        );
    }
    fn fixture(kernel: usize) -> Vec<u8> {
        // In-memory validator fixture ONLY; never saved as GPU evidence.
        let mut raw = vec![0xcd; GUARD * 2 + row_count(kernel) * STRIDE];
        for i in 0..row_count(kernel) {
            let p = GUARD + i * STRIDE;
            raw[p + 8..p + 12].copy_from_slice(&marker(tag(0, kernel), i).to_le_bytes());
            if kernel == 2 {
                raw[p + 4..p + 8].copy_from_slice(&ints()[i].to_le_bytes());
            }
        }
        for (i, value) in if kernel == 2 {
            vec![(0, 0u32), (1, 0x3c00), (2, 0xbc00)]
        } else if kernel == 0 {
            vec![(0, 0), (0x8000, 0), (0x3c00, 1), (0xbc00, 0xffff)]
        } else {
            vec![(0, 0), (0x8000, 0), (0x3c00, 1), (0xbc00, 0xffff_ffff)]
        } {
            let p = GUARD + i * STRIDE;
            if kernel == 1 {
                raw[p..p + 4].copy_from_slice(&value.to_le_bytes());
            } else {
                raw[p..p + 2].copy_from_slice(&(value as u16).to_le_bytes());
            }
        }
        raw
    }
    #[test]
    fn ordinary_rejects_actual_missing_row_shape() {
        for k in 0..3 {
            let mut raw = fixture(k);
            ordinary(&raw, 0, k).unwrap();
            let skip = if k == 2 { INT_SKIP } else { HALF_SKIP };
            let p = GUARD + skip * STRIDE;
            raw[p..p + STRIDE].fill(0xcd);
            assert_eq!(ordinary(&raw, 0, k).unwrap_err(), format!("missing marker row={skip}"));
        }
    }
    #[test]
    fn guards_and_reserved_fields_are_checked() {
        for (k, offset) in [(0, 0), (0, GUARD + 2), (0, GUARD + 6), (1, GUARD + 6), (2, GUARD + 2)] {
            let mut raw = fixture(k);
            raw[offset] ^= 1;
            assert!(ordinary(&raw, 0, k).is_err());
        }
    }
    #[test]
    fn nan_and_out_of_range_values_are_not_contract_gates() {
        let mut raw = fixture(0);
        for h in [0x7800usize, 0xf801, 0x7c00, 0x7e00, 0xffff] {
            let p = GUARD + h * STRIDE;
            raw[p..p + 2].copy_from_slice(&0x1234u16.to_le_bytes());
        }
        ordinary(&raw, 0, 0).unwrap();
    }
    #[test]
    fn air_needs_live_typed_call_not_just_declaration() {
        let ir="define void @convert_half_short() {\n %v = call i16 @air.convert.s.i16.f.f16(half %x)\n store i16 %v, ptr %out\n ret void\n}\n";
        prove_call(ir, "convert_half_short", "air.convert.s.i16.f.f16", "i16", "half").unwrap();
        assert!(prove_call(
            "declare i16 @air.convert.s.i16.f.f16(half)\n",
            "convert_half_short",
            "air.convert.s.i16.f.f16",
            "i16",
            "half"
        )
        .is_err());
        assert!(prove_call(
            &ir.replace("store i16 %v, ptr %out", "store i16 0, ptr %out"),
            "convert_half_short",
            "air.convert.s.i16.f.f16",
            "i16",
            "half"
        )
        .is_err());
        assert!(prove_call(
            &ir.replace("ret void", "%wide = fpext half %x to float\n ret void"),
            "convert_half_short",
            "air.convert.s.i16.f.f16",
            "i16",
            "half"
        )
        .is_err());
    }
}
