// Safe host adapter. Unit-test fixtures are NOT a native-output generation path.
#![forbid(unsafe_code)]
extern crate r16f_accept;
mod air;
use r16f_accept::*;
use std::{env, fs, io::Write, path::Path};
type Result<T> = std::result::Result<T, String>;
const HEADER: &str = "name\ttag\tlibrary_sha256\tinputs_sha256\ttexture\tpipeline\tpass\tclear_bits\tstatus\terror\tinputs_unchanged\tdraws\tread_dispatches\tgpu_start\tgpu_end\traw_sha256\ttrace_sha256\tread_sha256";
const TEXTURE: &str = "25,2,5,3,18,1,1,1,1,2,2,3,4,5,0,0,2";
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
        "{{\"name\":\"{}\",\"kind\":\"{}\",\"bits\":{},\"mask\":{},\"tag\":{},\"negative\":{}}}",
        s.name,kind(s.kind),s.bits,s.mask,s.tag,s.negative)
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\"schema\":1,\"rows\":{ROWS},\"width\":{WIDTH},\"height\":{HEIGHT},\"pitch\":{PITCH},\"guardBytes\":{GUARD},\"traceBytes\":{TRACE_BYTES},\"readBytes\":{READ_BYTES},\"captures\":[\n{entries}\n]}}\n")
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
    let b = read(&out.join("precise.metallib"))?;
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
    let binary = read(&out.join("precise.metallib"))?;
    let modules = air::extract(&binary)?;
    shape(&modules)?;
    let routes = text(&out.join("inspection.tsv"))?;
    let rows: Vec<_> = routes.lines().collect();
    need(
        rows.len() == 5 && rows[0] == "entry\troute",
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
        Kind::Clear | Kind::ClearZero | Kind::ClearNegativeZero => "2,1,0,0,0,0",
        _ => "1,1,0,0,0,0",
    }
}
fn clear_bits(s: Spec) -> &'static str {
    match s.kind {
        Kind::Seed => "none",
        Kind::Clear => "3fd8000000000000",
        Kind::ClearNegativeZero => "8000000000000000",
        _ => "0000000000000000",
    }
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
fn negative(out: &Path, raw: &[u8], trace: &[u8], reads: &[u8]) -> Result<()> {
    let s = SPECS[13];
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
fn hex_model(bits: u32, rne: bool) -> String {
    half_model(bits, rne).map_or_else(|| "outside-model".into(), |h| format!("{h:04x}"))
}
fn observations(s: Spec, raw: &[u8], trace: &[u8], reads: &[u8]) -> String {
    let mut t = String::new();
    for (i, input) in SOURCE_R.into_iter().enumerate() {
        let h = short(raw, offset(1, i + 1));
        let class = match i {
            0..=3 => "exact-anchor",
            4..=5 => "signed-zero",
            6..=11 => "half-midpoint",
            12..=13 => "finite-extreme",
            _ => "finite-end-tie",
        };
        let traced = if s.bits == 0 || (s.negative && i == 0) {
            "not-written".into()
        } else {
            format!("{:08x}", word(trace, 64 + i * 32 + 16))
        };
        // Competing models are diagnostic columns only; validation never uses
        // these floating-point expressions to prescribe hardware rounding.
        let model = if s.kind == Kind::Blend && traced != "not-written" {
            let bits = word(trace, 64 + i * 32 + 16);
            let source = f32::from_bits(if s.bits == 16 {
                expand(bits as u16)
            } else {
                bits
            });
            let product = f32::from_bits(expand(DEST_R[i])) * (1.0 - source);
            (source + product).to_bits()
        } else {
            input
        };
        let p = 64 + ((i + 1) * WIDTH + 1) * 48;
        t+=&format!("{}\t{}\t{i}\t{class}\t{input:08x}\t{:08x}\t{:04x}\t{traced}\t{h:04x}\t{:08x}\t{:08x}\t{}\t{}\n",
            s.name,s.bits,source(i)[3],DEST_R[i],word(reads,p),word(reads,p+16),hex_model(model,true),hex_model(model,false));
    }
    t
}
fn validate_all(out: &Path) -> Result<()> {
    need(
        read(&out.join("source.bin"))? == source_bytes()
            && read(&out.join("seed.bin"))? == seed()
            && text(&out.join("plan.json"))? == plan(),
        "synthetic inputs changed",
    )?;
    let metadata = text(&out.join("metadata.tsv"))?;
    need(
        metadata == "runner\tmacos-26\nos_build\t25G332\narchitecture\tarm64\nmode\tprecise\n"
            || metadata
                == "runner\txcode-27\nos_build\t26A434\narchitecture\tarm64\nmode\tprecise\n",
        "bounded reference cohort",
    )?;
    let (bound, proofs) = bindings(out)?;
    need(
        text(&out.join("bindings.tsv"))? == bound,
        "compiler bindings changed",
    )?;
    for (name, p) in proofs {
        need(text(&out.join(name))? == p, "AIR proof changed")?;
    }
    let library = air::sha(&read(&out.join("precise.metallib"))?);
    let receipts = text(&out.join("gpu.tsv"))?;
    let rows: Vec<_> = receipts.lines().collect();
    need(
        rows.len() == SPECS.len() + 1 && rows[0] == HEADER,
        "receipt header/coverage",
    )?;
    let mut measured=String::from("capture\treturn_bits\trow\tclass\tsource_r_f32\tsource_alpha_f32\tseed_r_f16\ttraced_return_r\tstored_r_f16\tread_r_f32\tread_r_f16\tdiagnostic_math_rne\tdiagnostic_math_rtz\n");
    for (i, s) in SPECS.into_iter().enumerate() {
        let raw = read(&out.join(format!("{}-raw.bin", s.name)))?;
        let trace = read(&out.join(format!("{}-trace.bin", s.name)))?;
        let reads = read(&out.join(format!("{}-read.bin", s.name)))?;
        receipt(
            rows[i + 1],
            s,
            &library,
            [&air::sha(&raw), &air::sha(&trace), &air::sha(&reads)],
        )?;
        if s.negative {
            negative(out, &raw, &trace, &reads)?;
        } else {
            validate(s, &raw, &trace, &reads).map_err(|e| format!("{}: {e:?}", s.name))?;
        }
        measured += &observations(s, &raw, &trace, &reads);
        println!(
            "{}: {}",
            s.name,
            if s.negative {
                "EXPECTED_TRACE_REJECTION_AND_SINGLE_SKIP_CONFIRMED"
            } else {
                "RAW_GATES_PASS"
            }
        );
    }
    let mut neg =
        String::from("control\tordinary_validator_result\nnative-single-skip\tTrace-rejected\n");
    for s in [SPECS[5], SPECS[6]] {
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
    println!("BOUNDED_RAW_AND_EXTRACTED_AIR_GATES_PASS; trusted compiler/runtime/hash chain still required; no universal FP claim");
    Ok(())
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let result = if args.len() != 3 {
        Err("usage: check generate|extract|bind|validate|audit PATH".into())
    } else {
        let p = Path::new(&args[2]);
        match args[1].as_str() {
            "generate" => generate(p),
            "extract" => extract(p),
            "bind" => bind(p),
            "validate" => validate_all(p),
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
        assert_eq!(source_bytes().len(), 256);
        assert_eq!(RAW_BYTES, 5120);
        assert_eq!(TRACE_BYTES, 640);
        assert_eq!(READ_BYTES, 3104);
        assert_eq!(input_hash().len(), 64);
        assert!(plan().len() < 4096);
    }
}
