// Safe host adapter. Unit-test fixtures are NOT a native-output generation path.
#![forbid(unsafe_code)]
extern crate r16f_accept;
mod air;
use r16f_accept::*;
use std::{env, fs, io::Write, path::Path};
type Result<T> = std::result::Result<T, String>;
const HEADER: &str = "name\ttag\tlibrary_sha256\tinputs_sha256\ttexture\tpipeline\tpass\tclear_bits\tstatus\terror\tinputs_unchanged\tdraws\tread_dispatches\tgpu_start\tgpu_end\traw_sha256\ttrace_sha256\tread_sha256";
const TEXTURE: &str = "25,2,5,3,66,1,1,1,1,2,2,3,4,5,0,0,2";
fn need(ok: bool, why: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(why.into())
    }
}
fn read(p: &Path) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(p).map_err(|_| "missing file")?;
    need(
        m.is_file() && !m.file_type().is_symlink() && m.len() <= 1_048_576,
        "file type/size",
    )?;
    let data = fs::read(p).map_err(|_| "file read")?;
    need(data.len() <= 1_048_576, "input grew beyond byte bound")?;
    Ok(data)
}
fn text(p: &Path) -> Result<String> {
    String::from_utf8(read(p)?).map_err(|_| "UTF-8".into())
}
fn save(p: &Path, data: &[u8]) -> Result<()> {
    need(data.len() <= 1_048_576, "write bound")?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .and_then(|mut f| f.write_all(data))
        .map_err(|_| "new file write".into())
}
fn kind(k: Kind) -> &'static str {
    match k {
        Kind::Seed => "seed",
        Kind::Clear => "clear",
        Kind::ClearZero => "clear-zero",
        Kind::ClearNegativeZero => "clear-negative-zero",
        Kind::ClearProbe => "clear-probe",
        Kind::Load => "load",
        Kind::Replace => "replace",
        Kind::Blend => "blend",
    }
}
fn source_bytes() -> Vec<u8> {
    (0..ROWS)
        .flat_map(|row| source(row).into_iter().flat_map(u32::to_le_bytes))
        .collect()
}
fn plan() -> String {
    let entries = SPECS
        .iter()
        .map(|s| {
            format!(
        "{{\"name\":\"{}\",\"kind\":\"{}\",\"bits\":{},\"mask\":{},\"tag\":{},\"negative\":{},\"clearBits\":\"{:016x}\"}}",
        s.name,kind(s.kind),s.bits,s.mask,s.tag,s.negative,clear_value(*s))
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\"schema\":2,\"rows\":{ROWS},\"width\":{WIDTH},\"height\":{HEIGHT},\"pitch\":{PITCH},\"guardBytes\":{GUARD},\"traceBytes\":{TRACE_BYTES},\"readBytes\":{READ_BYTES},\"captures\":[\n{entries}\n]}}\n")
}
fn input_hash() -> String {
    let mut b = plan().into_bytes();
    b.extend(source_bytes());
    b.extend(seed());
    air::sha(&b)
}
fn generate(out: &Path) -> Result<()> {
    save(&out.join("source.bin"), &source_bytes())?;
    save(&out.join("seed.bin"), &seed())?;
    save(&out.join("plan.json"), plan().as_bytes())?;
    save(&out.join("selection.tsv"), selection()?.as_bytes())?;
    save(&out.join("scope.tsv"), scope_table().as_bytes())?;
    println!("SYNTHETIC_INPUTS_ONLY rows={ROWS} captures={} target={WIDTH}x{HEIGHT}; no native execution",SPECS.len());
    Ok(())
}
fn shape(modules: &[air::Module<'_>]) -> Result<()> {
    need(
        modules.len() == air::ENTRIES.len(),
        "exact probe module count",
    )?;
    for (name, stage) in air::ENTRIES {
        need(
            modules
                .iter()
                .filter(|m| m.name == name && m.stage == stage)
                .count()
                == 1,
            "probe module/stage set",
        )?;
    }
    Ok(())
}
fn extract(out: &Path) -> Result<()> {
    let b = read(&out.join("program.metallib"))?;
    let modules = air::extract(&b)?;
    shape(&modules)?;
    println!("entry\tstage\tlibrary_offset\tbytes\tair_sha256\tlibrary_sha256");
    for m in modules {
        save(&out.join(format!("{}.air", m.name)), m.bytes)?;
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            m.name,
            m.stage,
            m.offset,
            m.bytes.len(),
            air::sha(m.bytes),
            air::sha(&b)
        );
    }
    Ok(())
}
fn bindings(out: &Path) -> Result<(String, Vec<(String, String)>)> {
    let binary = read(&out.join("program.metallib"))?;
    let modules = air::extract(&binary)?;
    shape(&modules)?;
    let routes = text(&out.join("inspection.tsv"))?;
    let rows: Vec<_> = routes.lines().collect();
    need(
        rows.len() == air::ENTRIES.len() + 1 && rows[0] == "entry\troute",
        "inspection route table",
    )?;
    let mut table = String::from(
        "entry\tstage\toffset\tbytes\tair_sha256\tll_sha256\tlibrary_sha256\tinspection_route\n",
    );
    let mut proofs = Vec::new();
    for (i, (name, stage)) in air::ENTRIES.into_iter().enumerate() {
        let m = modules.iter().find(|m| m.name == name).ok_or("module")?;
        let route: Vec<_> = rows[i + 1].split('\t').collect();
        need(
            route.len() == 2
                && route[0] == name
                && ["llvm-dis-extracted-air", "metal-ir-reader-extracted-air"].contains(&route[1]),
            "exact extracted-AIR route",
        )?;
        need(
            read(&out.join(format!("{name}.air")))? == m.bytes,
            "AIR is not the exact library subrange",
        )?;
        let ir = text(&out.join(format!("{name}.ll")))?;
        proofs.push((format!("{name}-proof.txt"), air::proof(&ir, name, stage)?));
        table += &format!(
            "{name}\t{stage}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            m.offset,
            m.bytes.len(),
            air::sha(m.bytes),
            air::sha(ir.as_bytes()),
            air::sha(&binary),
            route[1]
        );
    }
    Ok((table, proofs))
}
fn bind(out: &Path) -> Result<()> {
    let (table, proofs) = bindings(out)?;
    for (name, p) in proofs {
        save(&out.join(name), p.as_bytes())?;
    }
    save(&out.join("bindings.tsv"), table.as_bytes())?;
    println!("EXTRACTED_AIR_GATES_PASS; loaded-library runtime binding still required");
    Ok(())
}
fn pipeline(s: Spec) -> String {
    if s.bits == 0 {
        return "none".into();
    }
    if s.kind == Kind::Blend {
        format!("{},1,1,3,0,0,0,0,1,0,0,1", s.mask)
    } else {
        format!("{},0,1,0,0,1,0,0,1,0,0,1", s.mask)
    }
}
fn pass(s: Spec) -> &'static str {
    match s.kind {
        Kind::Seed => "none",
        Kind::Clear | Kind::ClearZero | Kind::ClearNegativeZero | Kind::ClearProbe => "2,1,0,0,0,0",
        _ => "1,1,0,0,0,0",
    }
}
fn clear_bits(s: Spec) -> String {
    if s.kind == Kind::Seed {
        "none".into()
    } else {
        format!("{:016x}", clear_value(s))
    }
}
fn mode_index(out: &Path) -> Result<u32> {
    let metadata = text(&out.join("metadata.tsv"))?;
    for (i, mode) in MODES.into_iter().enumerate() {
        for (label, build) in [("macos-26", "25G332"), ("xcode-27", "26A434")] {
            if metadata
                == format!(
                    "runner\t{label}\nos_build\t{build}\narchitecture\tarm64\nmode\t{mode}\n"
                )
            {
                return Ok(i as u32);
            }
        }
    }
    Err("bounded reference/mode metadata".into())
}
fn receipt(row: &str, s: Spec, library: &str, hashes: [&str; 3]) -> Result<()> {
    let f: Vec<_> = row.split('\t').collect();
    need(f.len() == 18, "receipt columns")?;
    need(
        f[0] == s.name && f[1] == s.tag.to_string(),
        "receipt identity/order",
    )?;
    need(
        f[2] == library && f[3] == input_hash(),
        "loaded-library/input hash binding",
    )?;
    need(
        f[4] == TEXTURE && f[5] == pipeline(s) && f[6] == pass(s) && f[7] == clear_bits(s),
        "actual texture/pipeline/pass receipt",
    )?;
    need(
        f[8..11] == ["4", "0", "1"] && f[11] == u32::from(s.bits != 0).to_string() && f[12] == "1",
        "command status/input/draw/dispatch receipt",
    )?;
    let start: f64 = f[13].parse().map_err(|_| "GPU start time")?;
    let end: f64 = f[14].parse().map_err(|_| "GPU end time")?;
    need(
        start.is_finite() && end.is_finite() && start >= 0.0 && end >= start,
        "GPU timestamp range",
    )?;
    need(f[15..18] == hashes, "raw output hash binding")
}
fn negative(out: &Path, mode: u32, raw: &[u8], trace: &[u8], reads: &[u8]) -> Result<()> {
    let s = mode_spec(SPECS[13], mode);
    let a = read(&out.join("f32-replace-raw.bin"))?;
    let b = read(&out.join("f32-replace-trace.bin"))?;
    need(
        a.len() == RAW_BYTES && b.len() == TRACE_BYTES,
        "negative reference lengths",
    )?;
    need(
        validate(s, raw, trace, reads) == Err(Error::Trace),
        "native skip must fail the ordinary validator specifically at Trace",
    )?;
    guards(raw, trace, reads).map_err(|e| format!("negative guards: {e:?}"))?;
    canonical(raw, reads, s.tag).map_err(|e| format!("negative canonical read: {e:?}"))?;
    need(
        trace[64..96].iter().all(|v| *v == POISON),
        "skipped trace row",
    )?;
    for row in 1..ROWS {
        let p = 64 + row * 32;
        if !row_enabled(s.tag, row) {
            need(
                trace[p..p + 32].iter().all(|v| *v == POISON),
                "excluded fast trace",
            )?;
            continue;
        }
        need(
            word(trace, p) == TRACE_MAGIC ^ s.tag && trace[p + 4..p + 32] == b[p + 4..p + 32],
            "unskipped trace differs",
        )?;
    }
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let want = if x == 1 && y == 1 {
                seed_half(x, y)
            } else {
                short(&a, offset(x, y))
            };
            need(
                short(raw, offset(x, y)) == want,
                "negative must skip exactly one pixel",
            )?;
        }
    }
    Ok(())
}
fn class16(h: u16) -> &'static str {
    match (h & 0x7c00, h & 0x3ff) {
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
        (0x7c00, _) => "nan-bits",
        _ => "normal",
    }
}
fn hex_half(h: Option<u16>) -> String {
    h.map_or_else(|| "nan".into(), |v| format!("{v:04x}"))
}
fn hex_model(bits: u32, rne: bool) -> String {
    hex_half(half_model(bits, rne))
}
fn observations(s: Spec, raw: &[u8], trace: &[u8], reads: &[u8]) -> String {
    let mut t = String::new();
    for i in 0..ROWS {
        let h = short(raw, offset(1, i + 1));
        let p = 64 + ((i + 1) * WIDTH + 1) * 48;
        let traced = if s.bits == 0 || (s.negative && i == 0) || !row_enabled(s.tag, i) {
            "not-written".into()
        } else {
            format!("{:08x}", word(trace, 64 + i * 32 + 16))
        };
        t += &format!(
            "{}\t{}\t{i}\t{}\t{:08x}\t{:08x}\t{:04x}\t{traced}\t{h:04x}\t{}\t{:08x}\t{:08x}\n",
            s.name,
            s.bits,
            CASES[i].0,
            SOURCE_R[i],
            source(i)[3],
            DEST_R[i],
            class16(h),
            word(reads, p),
            word(reads, p + 16)
        );
    }
    t
}
const SOURCE_MODELS: [&str; 3] = ["returned", "narrow-rne", "narrow-rtz"];
const FLUSH_MODELS: [&str; 3] = ["preserve", "flush-f32-input", "flush-half-input"];
const ARITHMETIC_MODELS: [&str; 9] = [
    "separate",
    "outer-fma",
    "expanded",
    "expanded-fma",
    "inner-fma",
    "half-steps-rne",
    "half-steps-rtz",
    "h-factor-fma-rne",
    "h-factor-fma-rtz",
];
fn narrow_value(x: f32, rne: bool) -> f32 {
    half_model(x.to_bits(), rne).map_or(f32::NAN, |h| f32::from_bits(expand(h)))
}
fn flushed(x: f32, domain: usize) -> f32 {
    let magnitude = x.to_bits() & 0x7fff_ffff;
    if magnitude != 0
        && magnitude
            < match domain {
                1 => 0x0080_0000,
                2 => 0x3880_0000,
                _ => 0,
            }
    {
        f32::from_bits(x.to_bits() & 0x8000_0000)
    } else {
        x
    }
}
fn arithmetic_model(s: f32, d: f32, op: usize) -> f32 {
    let factor = 1.0 - s;
    let product = d * factor;
    match op {
        0 => s + product,
        1 => d.mul_add(factor, s),
        2 => (s + d) - (s * d),
        3 => (-d).mul_add(s, s + d),
        4 => (-d).mul_add(s, d) + s,
        _ => {
            let rne = op == 5 || op == 7;
            let s = narrow_value(s, rne);
            let d = narrow_value(d, rne);
            let k = narrow_value(1.0 - s, rne);
            let p = narrow_value(d * k, rne);
            if op >= 7 {
                d.mul_add(k, s)
            } else {
                s + p
            }
        }
    }
}
fn selection() -> Result<String> {
    let mut t=String::from("row\tclass\tdomain\tsource_f32\tdestination_f16\tseparate_rne\touter_fma_rne\thalf_steps_rne\thalf_factor_fma_rne\tnarrow_source_rne\tseparate_rtz\touter_fma_rtz\tinner_fma_rtz\n");
    for row in 0..ROWS {
        let s = f32::from_bits(SOURCE_R[row]);
        let d = f32::from_bits(expand(DEST_R[row]));
        let plain = half_model(arithmetic_model(s, d, 0).to_bits(), true);
        let fused = half_model(arithmetic_model(s, d, 1).to_bits(), true);
        let half = half_model(arithmetic_model(s, d, 5).to_bits(), true);
        let narrowed = half_model(
            arithmetic_model(narrow_value(s, true), d, 0).to_bits(),
            true,
        );
        if (42..=45).contains(&row) {
            need(plain != fused, "fused/separate witness collapsed")?;
        }
        if [34, 46, 47].contains(&row) {
            need(
                half != prediction(s, d, 7, true, false),
                "owned half-fusion witness collapsed",
            )?;
        }
        if (52..58).contains(&row) {
            need(
                plain != half && DEST_R[row] & 1023 != 0,
                "normal rounded arithmetic witness collapsed",
            )?;
        }
        if row >= 58 {
            // Normal finite source/destination/output witnesses selected for
            // disagreements AFTER RTZ16 export, not merely at an f32/RNE step.
            const EXPECTED: [[u16; 3]; 6] = [
                [0x2cd1, 0x2cd0, 0x2cd0], [0x0aa8, 0x0aa8, 0x0aa9],
                [0x3add, 0x3ade, 0x3ade], [0xa256, 0xa255, 0xa256],
                [0xa995, 0xa995, 0xa996], [0x05ef, 0x05ee, 0x05ef],
            ];
            let got = [0, 1, 4].map(|op| prediction(s, d, op, false, false));
            need(got == EXPECTED[row - 58].map(Some), "RTZ16 order witness collapsed")?;
        }
        if row == 39 {
            need(plain != narrowed, "source precision witness collapsed")?;
        }
        t += &format!(
            "{row}\t{}\t{}\t{:08x}\t{:04x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            CASES[row].0,
            domain(row),
            SOURCE_R[row],
            DEST_R[row],
            hex_half(plain),
            hex_half(fused),
            hex_half(half),
            hex_half(prediction(s, d, 7, true, false)),
            hex_half(narrowed),
            hex_model(arithmetic_model(s, d, 0).to_bits(), false),
            hex_half(prediction(s, d, 1, false, false)),
            hex_half(prediction(s, d, 4, false, false))
        );
    }
    need(
        half_model(
            arithmetic_model(
                f32::from_bits(SOURCE_R[48]),
                f32::from_bits(expand(DEST_R[48])),
                0,
            )
            .to_bits(),
            true,
        ) == Some(0x0040),
        "normal-input underflow witness",
    )?;
    Ok(t)
}
fn prediction(s: f32, d: f32, op: usize, rne: bool, ftz: bool) -> Option<u16> {
    if op < 7 {
        return export(arithmetic_model(s, d, op).to_bits(), rne, ftz);
    }
    let hr = op == 7;
    let hs = half_model(s.to_bits(), hr)?;
    let hd = half_model(d.to_bits(), hr)?;
    let hk = half_model((1.0 - f32::from_bits(expand(hs))).to_bits(), hr)?;
    let h = if [hs, hd, hk].iter().all(|v| v & 0x7c00 != 0x7c00) {
        half_fma(hs, hd, hk, rne)
    } else {
        half_model(
            f32::from_bits(expand(hd))
                .mul_add(f32::from_bits(expand(hk)), f32::from_bits(expand(hs)))
                .to_bits(),
            rne,
        )
    };
    h.map(|v| {
        if ftz && v & 0x7c00 == 0 {
            v & 0x8000
        } else {
            v
        }
    })
}
fn scope_table() -> String {
    let mut t = String::from("row\tclass\tdomain\tfast_executed\tscope\n");
    for row in 0..ROWS {
        t += &format!(
            "{row}\t{}\t{}\t{}\t{}\n",
            CASES[row].0,
            domain(row),
            u8::from(row_enabled(256, row)),
            if !row_enabled(256, row) {
                "precise-only-overflow-experiment-not-general-qualification"
            } else if domain(row) == "normal-input-core" {
                "normal-input-core-observation"
            } else {
                "explicit-frontier-observation"
            }
        );
    }
    t+="capture\tclear-overflow\toverflow-observation\t0\tprecise-only-clear-nonfinite-experiment\n";
    t
}
fn export(bits: u32, rne: bool, ftz: bool) -> Option<u16> {
    half_model(bits, rne).map(|h| {
        if ftz && h & 0x7c00 == 0 {
            h & 0x8000
        } else {
            h
        }
    })
}
fn matches_model(actual: u16, want: Option<u16>) -> bool {
    want.map_or(actual & 0x7c00 == 0x7c00 && actual & 1023 != 0, |v| {
        v == actual
    })
}
fn hypotheses(s: Spec, raw: &[u8], trace: &[u8]) -> (String, String) {
    let mut detail = String::new();
    let mut summary = String::new();
    for sm in 0..3 {
        for fm in 0..3 {
            for op in 0..ARITHMETIC_MODELS.len() {
                let mut mismatch = [0usize; 4];
                let mut core_mismatch = [0usize; 4];
                let mut first_core = [None; 4];
                let active_rows = (0..ROWS).filter(|&row| row_enabled(s.tag, row)).count();
                let core_rows = (0..ROWS)
                    .filter(|&r| domain(r) == "normal-input-core")
                    .count();
                let mut first = [None; 4];
                let mut nan_class = [0usize; 4];
                for row in 0..ROWS {
                    if !row_enabled(s.tag, row) {
                        continue;
                    }
                    let bits = word(trace, 64 + row * 32 + 16);
                    let returned = f32::from_bits(if s.bits == 16 {
                        expand(bits as u16)
                    } else {
                        bits
                    });
                    let source = match sm {
                        0 => returned,
                        1 => narrow_value(returned, true),
                        _ => narrow_value(returned, false),
                    };
                    let d = f32::from_bits(expand(DEST_R[row]));
                    let value = arithmetic_model(flushed(source, fm), flushed(d, fm), op).to_bits();
                    let actual = short(raw, offset(1, row + 1));
                    let (sv, dv) = (flushed(source, fm), flushed(d, fm));
                    let candidates = [
                        prediction(sv, dv, op, true, false),
                        prediction(sv, dv, op, false, false),
                        prediction(sv, dv, op, true, true),
                        prediction(sv, dv, op, false, true),
                    ];
                    for i in 0..4 {
                        if !matches_model(actual, candidates[i]) {
                            mismatch[i] += 1;
                            if domain(row) == "normal-input-core" {
                                core_mismatch[i] += 1;
                                if first_core[i].is_none() {
                                    first_core[i] = Some(row);
                                }
                            }
                            if first[i].is_none() {
                                first[i] = Some(row);
                            }
                        } else if candidates[i].is_none() {
                            nan_class[i] += 1;
                        }
                    }
                    detail += &format!(
                        "{}\t{row}\t{}\t{}\t{}\t{}\t{actual:04x}\t{}\t{}\t{}\t{}\n",
                        s.name,
                        SOURCE_MODELS[sm],
                        FLUSH_MODELS[fm],
                        ARITHMETIC_MODELS[op],
                        if op >= 7 {
                            "fma16".into()
                        } else {
                            format!("{value:08x}")
                        },
                        hex_half(candidates[0]),
                        hex_half(candidates[1]),
                        hex_half(candidates[2]),
                        hex_half(candidates[3])
                    );
                }
                for (i, e) in ["rne", "rtz", "rne-output-ftz", "rtz-output-ftz"]
                    .into_iter()
                    .enumerate()
                {
                    summary += &format!(
                        "{}\t{}\t{}\t{}\t{e}\t{active_rows}\t{core_rows}\t{}\t{}\t{}\t{}\t{}\n",
                        s.name,
                        SOURCE_MODELS[sm],
                        FLUSH_MODELS[fm],
                        ARITHMETIC_MODELS[op],
                        core_mismatch[i],
                        mismatch[i],
                        nan_class[i],
                        first_core[i].map_or_else(|| "none".into(), |n| n.to_string()),
                        first[i].map_or_else(|| "none".into(), |n| n.to_string())
                    );
                }
            }
        }
    }
    (detail, summary)
}
fn native_math(out: &Path, mode: u32, library: &str) -> Result<String> {
    let receipt = text(&out.join("math.tsv"))?;
    let rows: Vec<_> = receipt.lines().collect();
    need(rows.len()==3&&rows[0]=="mode\tprecision\tlibrary_sha256\tinputs_sha256\tstatus\terror\tinputs_unchanged\tgrid\tthreads_per_group\tgpu_start\tgpu_end\tsha256","math receipt schema")?;
    let mut result=String::from("precision\trow\tclass\ts_native_f32\td_raw_f16\td_native_f32\tfactor\tproduct\tplain_expression\texplicit_outer_fma\texpanded_expression\texpanded_fma\ts_native_half\tplain_native_half\tfma_native_half\tinput_f32_echo\thalf_factor\thalf_product\thalf_plain\thalf_explicit_fma\thalf_plain_as_f32\thalf_fma_as_f32\thalf_marker\td_half_echo\n");
    for (i, bits) in [32, 16].into_iter().enumerate() {
        let raw = read(&out.join(format!("math-f{bits}.bin")))?;
        math_capture(&raw, mode, bits).map_err(|e| format!("math f{bits}: {e:?}"))?;
        let c: Vec<_> = rows[i + 1].split('\t').collect();
        need(
            c.len() == 12
                && c[0] == MODES[mode as usize]
                && c[1] == bits.to_string()
                && c[2] == library
                && c[3] == input_hash()
                && c[4..7] == ["4", "0", "1"]
                && c[7] == (ROWS + 4).to_string()
                && c[8] == "32"
                && c[11] == air::sha(&raw),
            "math runtime/hash binding",
        )?;
        let a: f64 = c[9].parse().map_err(|_| "math start")?;
        let b: f64 = c[10].parse().map_err(|_| "math end")?;
        need(
            a.is_finite() && b.is_finite() && a >= 0.0 && b >= a,
            "math GPU times",
        )?;
        for row in 0..ROWS {
            if !row_enabled(mode << 8, row) {
                continue;
            }
            result += &format!("{bits}\t{row}\t{}", CASES[row].0);
            for lane in 3..24 {
                result += &format!("\t{:08x}", word(&raw, 64 + row * MATH_STRIDE + lane * 4));
            }
            result.push('\n');
        }
    }
    Ok(result)
}
fn compare_modes(out: &Path) -> Result<()> {
    let mut delta = String::from(
        "capture\trow\tclass\tprecise_r16\tfast_r16\tprecise_return\tfast_return\tstored_equal\n",
    );
    for s in SPECS {
        if !capture_enabled(s, 1) {
            continue;
        }
        let pa = read(&out.join(format!("precise/{}-raw.bin", s.name)))?;
        let fa = read(&out.join(format!("fast/{}-raw.bin", s.name)))?;
        let pt = read(&out.join(format!("precise/{}-trace.bin", s.name)))?;
        let ft = read(&out.join(format!("fast/{}-trace.bin", s.name)))?;
        need(
            pa.len() == RAW_BYTES
                && fa.len() == RAW_BYTES
                && pt.len() == TRACE_BYTES
                && ft.len() == TRACE_BYTES,
            "comparison lengths",
        )?;
        for row in 0..ROWS {
            let a = short(&pa, offset(1, row + 1));
            let b = short(&fa, offset(1, row + 1));
            let (pr, fr) = if s.bits == 0 || (s.negative && row == 0) {
                ("not-written".to_string(), "not-written".to_string())
            } else {
                (
                    format!("{:08x}", word(&pt, 64 + row * 32 + 16)),
                    if row_enabled(256, row) {
                        format!("{:08x}", word(&ft, 64 + row * 32 + 16))
                    } else {
                        "excluded".into()
                    },
                )
            };
            delta += &format!(
                "{}\t{row}\t{}\t{a:04x}\t{b:04x}\t{pr}\t{fr}\t{}\n",
                s.name,
                CASES[row].0,
                u8::from(a == b)
            );
        }
    }
    save(&out.join("mode-differences.tsv"), delta.as_bytes())?;
    let mut math = String::from("precision\trow\tclass\tlane\tprecise_bits\tfast_bits\tequal\n");
    for bits in [32, 16] {
        let a = read(&out.join(format!("precise/math-f{bits}.bin")))?;
        let b = read(&out.join(format!("fast/math-f{bits}.bin")))?;
        need(
            a.len() == MATH_BYTES && b.len() == MATH_BYTES,
            "math comparison lengths",
        )?;
        for row in 0..ROWS {
            if !row_enabled(256, row) {
                continue;
            }
            for lane in 3..24 {
                let x = word(&a, 64 + row * MATH_STRIDE + lane * 4);
                let y = word(&b, 64 + row * MATH_STRIDE + lane * 4);
                math += &format!(
                    "{bits}\t{row}\t{}\t{lane}\t{x:08x}\t{y:08x}\t{}\n",
                    CASES[row].0,
                    u8::from(x == y)
                );
            }
        }
    }
    save(&out.join("math-mode-differences.tsv"), math.as_bytes())?;
    println!("MODE_COMPARISONS_RETAINED; numeric profile is NOT selected automatically");
    Ok(())
}
fn validate_all(out: &Path) -> Result<()> {
    need(
        read(&out.join("source.bin"))? == source_bytes()
            && read(&out.join("seed.bin"))? == seed()
            && text(&out.join("plan.json"))? == plan(),
        "synthetic inputs changed",
    )?;
    need(
        text(&out.join("selection.tsv"))? == selection()?,
        "software witness selection changed",
    )?;
    need(
        text(&out.join("scope.tsv"))? == scope_table(),
        "explicit scope policy changed",
    )?;
    let mode = mode_index(out)?;
    let (bound, proofs) = bindings(out)?;
    need(
        text(&out.join("bindings.tsv"))? == bound,
        "compiler bindings changed",
    )?;
    for (name, p) in proofs {
        need(text(&out.join(name))? == p, "AIR proof changed")?;
    }
    let library = air::sha(&read(&out.join("program.metallib"))?);
    let receipts = text(&out.join("gpu.tsv"))?;
    let rows: Vec<_> = receipts.lines().collect();
    need(
        rows.len() == SPECS.iter().filter(|s| capture_enabled(**s, mode)).count() + 1
            && rows[0] == HEADER,
        "receipt header/coverage",
    )?;
    let mut measured=String::from("capture\treturn_bits\trow\tclass\tsource_r_f32\tsource_alpha_f32\tseed_r_f16\ttraced_return_r\tstored_r_f16\tstored_class\tread_r_f32\tread_r_f16\n");
    let mut detail=String::from("capture\trow\tsource_model\tinput_flush\tarithmetic\tmodel_value_or_route\tactual_r16\trne\trtz\trne_output_ftz\trtz_output_ftz\n");
    let mut models=String::from("capture\tsource_model\tinput_flush\tarithmetic\texport\trows\tcore_rows\tcore_mismatches\tall_mismatches\tnan_class_matches\tfirst_core_mismatch\tfirst_any_mismatch\n");
    let mut clears=String::from("capture\tclear_f64\tvia_f32\tdirect_rne\tdirect_rtz\tf32_rne\tf32_rtz\tactual_r16\tclass\n");
    let mut failures = Vec::new();
    for (i, base) in SPECS
        .into_iter()
        .filter(|s| capture_enabled(*s, mode))
        .enumerate()
    {
        let s = mode_spec(base, mode);
        let raw = read(&out.join(format!("{}-raw.bin", s.name)))?;
        let trace = read(&out.join(format!("{}-trace.bin", s.name)))?;
        let reads = read(&out.join(format!("{}-read.bin", s.name)))?;
        receipt(
            rows[i + 1],
            s,
            &library,
            [&air::sha(&raw), &air::sha(&trace), &air::sha(&reads)],
        )?;
        guards(&raw, &trace, &reads).map_err(|e| format!("{} structural: {e:?}", s.name))?;
        let gate = if s.negative {
            negative(out, mode, &raw, &trace, &reads)
        } else {
            validate(s, &raw, &trace, &reads).map_err(|e| format!("{}: {e:?}", s.name))
        };
        if let Err(e) = gate {
            failures.push(e);
        }
        measured += &observations(s, &raw, &trace, &reads);
        if s.kind == Kind::Blend {
            let (d, m) = hypotheses(s, &raw, &trace);
            detail += &d;
            models += &m;
        }
        if matches!(
            s.kind,
            Kind::Clear | Kind::ClearZero | Kind::ClearNegativeZero | Kind::ClearProbe
        ) {
            let bits = clear_value(s);
            let via = (f64::from_bits(bits) as f32).to_bits();
            let h = short(&raw, offset(1, 1));
            clears += &format!(
                "{}\t{bits:016x}\t{via:08x}\t{}\t{}\t{}\t{}\t{h:04x}\t{}\n",
                s.name,
                hex_half(half64_model(bits, true)),
                hex_half(half64_model(bits, false)),
                hex_model(via, true),
                hex_model(via, false),
                class16(h)
            );
        }
        println!(
            "{}: {}",
            s.name,
            if s.negative {
                "EXPECTED_TRACE_REJECTION_AND_SINGLE_SKIP_CONFIRMED"
            } else {
                "CAPTURE_RETAINED_SEMANTIC_GATE_REPORTED_AT_END"
            }
        );
    }
    let mut neg =
        String::from("control\tordinary_validator_result\nnative-single-skip\tTrace-rejected\n");
    for base in [SPECS[5], SPECS[6]] {
        let s = mode_spec(base, mode);
        let raw = read(&out.join(format!("{}-raw.bin", s.name)))?;
        let trace = read(&out.join(format!("{}-trace.bin", s.name)))?;
        let reads = read(&out.join(format!("{}-read.bin", s.name)))?;
        need(
            validate(
                Spec {
                    kind: Kind::Blend,
                    ..s
                },
                &raw,
                &trace,
                &reads,
            ) == Err(Error::Anchor),
            "replacement must not qualify blending",
        )?;
        neg += &format!("{}-as-blend\tAnchor-rejected\n", s.name);
    }
    save(&out.join("observations.tsv"), measured.as_bytes())?;
    save(&out.join("negatives.tsv"), neg.as_bytes())?;
    save(&out.join("hypotheses.tsv"), detail.as_bytes())?;
    save(&out.join("model-matches.tsv"), models.as_bytes())?;
    save(&out.join("clear-observations.tsv"), clears.as_bytes())?;
    save(
        &out.join("native-math.tsv"),
        native_math(out, mode, &library)?.as_bytes(),
    )?;
    let mut precision =
        String::from("operation\trow\tclass\tf32_stored\tf16_stored\tf32_return\tf16_return\n");
    for operation in ["replace", "blend"] {
        let a = read(&out.join(format!("f32-{operation}-raw.bin")))?;
        let b = read(&out.join(format!("f16-{operation}-raw.bin")))?;
        let at = read(&out.join(format!("f32-{operation}-trace.bin")))?;
        let bt = read(&out.join(format!("f16-{operation}-trace.bin")))?;
        for row in 0..ROWS {
            if !row_enabled(mode << 8, row) {
                continue;
            }
            precision += &format!(
                "{operation}\t{row}\t{}\t{:04x}\t{:04x}\t{:08x}\t{:08x}\n",
                CASES[row].0,
                short(&a, offset(1, row + 1)),
                short(&b, offset(1, row + 1)),
                word(&at, 64 + row * 32 + 16),
                word(&bt, 64 + row * 32 + 16)
            );
        }
    }
    save(&out.join("source-precision.tsv"), precision.as_bytes())?;
    need(failures.is_empty(), &failures.join("; "))?;
    println!("CAPTURE_INTEGRITY_PASS_MATH_PROFILE_UNSELECTED; candidate matches are bounded observations, not a final default repair");
    Ok(())
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let result = if args.len() != 3 {
        Err("usage: check generate|extract|bind|validate|compare|audit PATH".into())
    } else {
        let p = Path::new(&args[2]);
        match args[1].as_str() {
            "generate" => generate(p),
            "extract" => extract(p),
            "bind" => bind(p),
            "validate" => validate_all(p),
            "compare" => compare_modes(p),
            "audit" => read(p).and_then(|b| {
                for m in air::extract(&b)? {
                    println!(
                        "{} stage={} offset={} bytes={} sha256={}",
                        m.name,
                        m.stage,
                        m.offset,
                        m.bytes.len(),
                        air::sha(m.bytes)
                    );
                }
                Ok(())
            }),
            _ => Err("unknown operation".into()),
        }
    };
    if let Err(e) = result {
        eprintln!("UNQUALIFIED: {e}");
        std::process::exit(3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(s: Spec) -> String {
        [
            s.name.to_string(),
            s.tag.to_string(),
            "a".repeat(64),
            input_hash(),
            TEXTURE.into(),
            pipeline(s),
            pass(s).into(),
            clear_bits(s).into(),
            "4".into(),
            "0".into(),
            "1".into(),
            u32::from(s.bits != 0).to_string(),
            "1".into(),
            "0".into(),
            "0".into(),
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64),
        ]
        .join("\t")
    }
    #[test]
    fn receipt_all_states_and_bad_bindings() {
        for s in SPECS {
            let r = row(s);
            assert!(receipt(
                &r,
                s,
                &"a".repeat(64),
                [&"b".repeat(64), &"c".repeat(64), &"d".repeat(64)]
            )
            .is_ok());
        }
        let s = SPECS[9];
        let good = row(s);
        for (i, bad) in [
            (0, "f16-blend"),
            (1, "99"),
            (2, "wrong"),
            (3, "wrong"),
            (4, "25,2,4"),
            (5, "15,1,1,5,0,0,0,0,1,0,0,1"),
            (6, "0,1,0,0,0,0"),
            (7, "8000000000000000"),
            (8, "3"),
            (9, "1"),
            (10, "0"),
            (11, "0"),
            (12, "0"),
            (13, "NaN"),
            (14, "-1"),
            (15, "wrong"),
            (16, "wrong"),
            (17, "wrong"),
        ] {
            let mut cols: Vec<_> = good.split('\t').collect();
            cols[i] = bad;
            assert!(
                receipt(
                    &cols.join("\t"),
                    s,
                    &"a".repeat(64),
                    [&"b".repeat(64), &"c".repeat(64), &"d".repeat(64)]
                )
                .is_err(),
                "column {i}"
            );
        }
    }
    #[test]
    fn fixed_plan_layout_and_scope() {
        assert_eq!(SPECS.iter().filter(|s| s.bits != 0).count(), 9);
        assert_eq!(SPECS.iter().filter(|s| s.kind == Kind::Blend).count(), 2);
        assert_eq!(SPECS.iter().filter(|s| s.negative).count(), 1);
        assert_eq!(source_bytes().len(), 1024);
        assert_eq!(RAW_BYTES, 17408);
        assert_eq!(TRACE_BYTES, 2176);
        assert_eq!(READ_BYTES, 10016);
        assert_eq!(input_hash().len(), 64);
        assert!(plan().len() < 4096);
    }
    #[test]
    fn selected_rows_really_discriminate_fusion_and_export() {
        for row in [42, 43, 44, 45] {
            let s = f32::from_bits(SOURCE_R[row]);
            let d = f32::from_bits(expand(DEST_R[row]));
            assert_ne!(
                half_model(arithmetic_model(s, d, 0).to_bits(), true),
                half_model(arithmetic_model(s, d, 1).to_bits(), true),
                "row {row}"
            );
        }
        assert_ne!(
            half_model(SOURCE_R[8], true),
            half_model(SOURCE_R[8], false)
        );
        let s = f32::from_bits(SOURCE_R[39]);
        let d = 2.0;
        assert_ne!(
            half_model(arithmetic_model(s, d, 0).to_bits(), true),
            half_model(
                arithmetic_model(narrow_value(s, true), d, 0).to_bits(),
                true
            )
        );
        assert_ne!(
            half64_model(clear_value(SPECS[15]), true),
            half64_model(clear_value(SPECS[16]), true)
        );
    }
    #[test]
    fn diagnostic_models_are_bounded_and_do_not_clamp_source() {
        assert_eq!(arithmetic_model(-2.0, -4.0, 0), -14.0);
        assert_eq!(arithmetic_model(2.0, 0.25, 0), 1.75);
        assert_eq!(arithmetic_model(-128.0, 512.0, 0), 65920.0);
        assert_eq!(class16(0xfc00), "-infinity");
        assert_eq!(class16(0x8000), "-zero");
        let s = SPECS[9];
        let raw = seed();
        let mut trace = vec![0xcd; TRACE_BYTES];
        for row in 0..ROWS {
            trace[64 + row * 32 + 16..64 + row * 32 + 20]
                .copy_from_slice(&SOURCE_R[row].to_le_bytes());
        }
        let (d, m) = hypotheses(s, &raw, &trace);
        assert!(d.len() * 2 + 256 <= 1_048_576);
        assert!(m.len() * 2 + 256 <= 1_048_576);
        assert_eq!(d.lines().count(), ROWS * 3 * 3 * ARITHMETIC_MODELS.len());
        assert_eq!(m.lines().count(), 3 * 3 * ARITHMETIC_MODELS.len() * 4);
    }
    #[test]
    fn complete_selection_and_core_partition() {
        assert_eq!(half64_model(clear_value(SPECS[20]), true), Some(0x3c02));
        assert_eq!(half64_model(clear_value(SPECS[20]), false), Some(0x3c01));
        let text = selection().unwrap();
        assert_eq!(text.lines().count(), ROWS + 1);
        assert_eq!(
            (0..ROWS)
                .filter(|&r| domain(r) == "normal-input-core")
                .count(),
            37
        );
        for row in 48..=51 {
            assert!(normal(DEST_R[row]));
            assert!(f32::from_bits(SOURCE_R[row]).is_normal());
        }
        for row in 52..=57 {
            assert_eq!(
                f32::from_bits(expand(half_model(SOURCE_R[row], true).unwrap())).to_bits(),
                SOURCE_R[row]
            );
        }
    }
}
