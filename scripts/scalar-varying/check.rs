// Host-only file/AIR adapter. Acceptance arithmetic itself is safe no_std Rust.
#![forbid(unsafe_code)]
extern crate scalar_accept;
use scalar_accept::*;
use std::{env, fs, io::Write, path::Path};
type Result<T> = std::result::Result<T, String>;
fn need(ok: bool, why: &str) -> Result<()> { if ok { Ok(()) } else { Err(why.into()) } }
fn read(p: &Path) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(p).map_err(|_| "missing file")?;
    need(m.is_file() && !m.file_type().is_symlink() && m.len() <= 1_048_576, "file type/size")?;
    fs::read(p).map_err(|_| "file read".into())
}
fn text(p: &Path) -> Result<String> { String::from_utf8(read(p)?).map_err(|_| "UTF-8".into()) }
fn save(p: &Path, bytes: &[u8]) -> Result<()> {
    need(bytes.len() <= 1_048_576, "write bound")?;
    fs::OpenOptions::new().write(true).create_new(true).open(p).and_then(|mut f| f.write_all(bytes)).map_err(|_| "new file write".into())
}
fn body<'a>(ir: &'a str, entry: &str) -> Result<&'a str> {
    let needle = format!("@{entry}(");
    let quoted = format!("@\"{entry}\"(");
    let mut found = None;
    let mut offset = 0;
    for line in ir.split_inclusive('\n') {
        if line.starts_with("define ") && (line.contains(&needle) || line.contains(&quoted)) {
            need(found.is_none(), "duplicate entry")?;
            let tail = &ir[offset..];
            found = Some(&tail[..tail.find("\n}").ok_or("unterminated definition")? + 2]);
        }
        offset += line.len();
    }
    found.ok_or_else(|| format!("missing definition {entry}"))
}
fn assignment<'a>(body: &'a str, value: &str) -> Option<&'a str> {
    body.lines().filter_map(|l| l.trim().split_once(" = ")).find_map(|(lhs, rhs)| (lhs == value).then_some(rhs))
}
fn scalar_proof(ir: &str) -> Result<String> {
    let b = body(ir, "scalar_half_ushort")?;
    let needle = "@air.convert.u.i16.f.f16(";
    need(!b.contains("fpext") && !b.contains("fptrunc") && !b.contains("fptoui"), "substituted conversion route")?;
    let calls: Vec<_> = b.lines().filter(|l| l.contains("call ") && l.contains(needle)).collect();
    need(calls.len() == 1, "one live scalar unsigned half-to-i16 call required")?;
    need(!b.lines().any(|l| l.contains("air.convert.") && !l.contains(needle)), "other conversion route")?;
    let (result, call) = calls[0].trim().split_once(" = ").ok_or("unnamed result")?;
    let (before, after) = call.split_once(needle).ok_or("callee")?;
    let args = after.split_once(')').ok_or("call arguments")?.0;
    let parts: Vec<_> = args.split_whitespace().collect();
    need(result.starts_with('%') && before.split_whitespace().last() == Some("i16") &&
         parts.len() == 2 && parts[0] == "half" && parts[1].starts_with('%'), "scalar call type/value")?;
    let input = assignment(b, parts[1]).ok_or("input SSA definition")?;
    let load = if let Some(cast) = input.strip_prefix("bitcast i16 ") {
        let (source, suffix) = cast.split_once(" to half").ok_or("half bitcast")?;
        need(suffix.is_empty() || suffix.starts_with(','), "bitcast suffix")?;
        let load = assignment(b, source).ok_or("bitcast source SSA")?;
        need(load.starts_with("load i16,"), "input i16 load")?; load
    } else { need(input.starts_with("load half,"), "dynamic scalar half load")?; input };
    need(load.contains("addrspace(1)"), "input must be a device-buffer load")?;
    let store = b.lines().find(|l| {
        let fields: Vec<_> = l.split_whitespace().collect();
        fields.len() >= 4 && fields[0] == "store" && fields[1] == "i16" &&
            fields[2].strip_suffix(',') == Some(result) && l.contains("addrspace(1)")
    }).ok_or("live scalar result device store")?;
    let declarations: Vec<_> = ir.lines().filter(|l| l.starts_with("declare ") && l.contains(needle)).collect();
    need(declarations.len() == 1, "unique exact declaration")?;
    let (prefix, suffix) = declarations[0].split_once(needle).ok_or("declaration")?;
    need(prefix.split_whitespace().last() == Some("i16") && suffix.split_once(')').map(|p| p.0) == Some("half"), "declaration signature")?;
    Ok(format!("SCALAR_AIR_PASS\n{}\n{}\n{}\n{}\n", load.trim(), calls[0].trim(), store.trim(), declarations[0]))
}
fn refs(s: &str) -> Vec<&str> {
    let b = s.as_bytes(); let mut out = Vec::new(); let mut i = 0; let mut quoted = false;
    while i < b.len() {
        if b[i] == b'"' { quoted = !quoted; }
        if !quoted && b[i] == b'!' && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let start = i; i += 1;
            while b.get(i).is_some_and(u8::is_ascii_digit) { i += 1; }
            out.push(&s[start..i]); continue;
        }
        i += 1;
    }
    out
}
fn node<'a>(ir: &'a str, id: &str) -> Result<&'a str> {
    let prefix = format!("{id} = ");
    let v: Vec<_> = ir.lines().filter_map(|l| l.strip_prefix(&prefix)).collect();
    need(v.len() == 1, "missing/duplicate metadata node")?; Ok(v[0])
}
fn stage<'a>(ir: &'a str, root: &str, entry: &str) -> Result<Vec<&'a str>> {
    let entry_name = format!("@{entry},"); let quoted = format!("@\"{entry}\",");
    let candidates: Vec<_> = refs(node(ir, root)?).iter().map(|id| node(ir, id)).collect::<Result<Vec<_>>>()?
        .into_iter().filter(|v| v.contains(&entry_name) || v.contains(&quoted)).collect();
    need(candidates.len() == 1, "unique stage metadata entry")?;
    let mut todo = refs(candidates[0]); let mut seen = Vec::new(); let mut out = Vec::new();
    while let Some(id) = todo.pop() {
        if seen.contains(&id) { continue; }
        need(seen.len() < 128, "metadata traversal bound")?;
        seen.push(id); let value = node(ir, id)?; out.push(value); todo.extend(refs(value));
    }
    Ok(out)
}
fn location(record: &str) -> Result<&str> {
    // Both explicit user locations and compiler-generated structural links
    // are identities. Preserve the full spelling when joining the two stages.
    let identities: Vec<_> = record.split("!\"").skip(1)
        .filter_map(|part| part.split_once('"').map(|v| v.0))
        .filter(|value| value.starts_with("user(") || value.starts_with("generated("))
        .collect();
    need(identities.len() == 1, "unique linkage identity")?;
    let identity = identities[0];
    let payload = identity.split_once('(').ok_or("linkage opening")?.1
        .strip_suffix(')').ok_or("linkage closing")?;
    need(!payload.is_empty() && payload.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'), "linkage payload")?;
    Ok(identity)
}
fn varying_proof(ir: &str) -> Result<String> {
    for entry in ["large_vertex", "large_fragment"] {
        let b = body(ir, entry)?;
        need(!b.contains("half") && !b.contains("fptrunc"), "half route in linked-f32 body")?;
    }
    let vertex: Vec<_> = stage(ir, "!air.vertex", "large_vertex")?.into_iter()
        .filter(|l| l.contains("!\"air.vertex_output\"") && l.contains("!\"float3\"")).collect();
    let fragment: Vec<_> = stage(ir, "!air.fragment", "large_fragment")?.into_iter()
        .filter(|l| l.contains("!\"air.fragment_input\"") && l.contains("!\"float3\"")).collect();
    need(vertex.len() == 2 && fragment.len() == 2, "two genuine f32 float3 stage links")?;
    let mut vl = vertex.iter().map(|l| location(l)).collect::<Result<Vec<_>>>()?; vl.sort();
    let mut fl = fragment.iter().map(|l| location(l)).collect::<Result<Vec<_>>>()?; fl.sort();
    need(vl == fl && vl[0] != vl[1], "vertex/fragment user location join")?;
    need(fragment.iter().all(|l| l.contains("!\"air.center\"")) &&
         fragment.iter().filter(|l| l.contains("!\"air.perspective\"")).count() == 1 &&
         fragment.iter().filter(|l| l.contains("!\"air.no_perspective\"")).count() == 1, "explicit interpolation qualifiers")?;
    Ok(format!("LINKED_F32_AIR_PASS\n{}\n{}\n", vertex.join("\n"), fragment.join("\n")))
}
fn air(source: &str) -> Result<String> {
    // Narrow compiler-text gate, not a general LLVM parser. Unknown spellings fail.
    let ir = source.lines().map(|l| l.split(';').next().unwrap_or("").trim_end()).collect::<Vec<_>>().join("\n");
    let mut proof = scalar_proof(&ir)?; proof.push_str(&varying_proof(&ir)?);
    proof.push_str("ACTUAL_COMPILER_MATH_METADATA (absence is not preservation proof):\n");
    for l in ir.lines().filter(|l| l.contains("air.compile") || l.contains("denormal") || l.contains("unsafe-fp") || l.contains("no-nans-fp")) {
        proof.push_str(l); proof.push('\n');
    }
    Ok(proof)
}
fn generate(out: &Path) -> Result<()> {
    let half: Vec<u8> = CASES.iter().flat_map(|&(h, _)| h.to_le_bytes()).collect();
    save(&out.join("half.bin"), &half)?;
    let mut vertices = Vec::new();
    let mut table = String::from("case\tvertex\tclip_x\tclip_y\tclip_z\tclip_w\tvalue_bits\tbasis1\tbasis2\n");
    for case in 0..2 { for i in 0..3 {
        let w = [1.0f32, 2.0, 4.0][i]; let x = [-1.0, 3.0, -1.0][i] * w; let y = [1.0, 1.0, -3.0][i] * w;
        let value = BASE + if case == 0 { 0.0 } else { [0.0, 1024.0, -512.0][i] };
        let b1 = if i == 1 { 1.0f32 } else { 0.0 }; let b2 = if i == 2 { 1.0f32 } else { 0.0 };
        vertices.extend([x, y, 0.5 * w, w, value, b1, b2, 0.0].into_iter().flat_map(|v| v.to_le_bytes()));
        table += &format!("{case}\t{i}\t{x}\t{y}\t{}\t{w}\t{:08x}\t{b1}\t{b2}\n", 0.5*w, value.to_bits());
    }}
    save(&out.join("vertices.bin"), &vertices)?; save(&out.join("vertices.tsv"), table.as_bytes())?;
    let mut expected = String::from("row\thalf_bits\tu16_bits\tzero_extended_bits\n");
    for (i, &(h, v)) in CASES.iter().enumerate() { need(rtz_half(h) == Some(v), "input table oracle")?; expected += &format!("{i}\t{h:04x}\t{v:04x}\t{:08x}\n", u32::from(v)); }
    save(&out.join("expected-u16.tsv"), expected.as_bytes())?;
    println!("GENERATED 20 valid half inputs; 2 unequal-W triangles; no native execution"); Ok(())
}
fn receipts(bindings: &str, gpu: &str) -> Result<()> {
    let binding: Vec<_> = bindings.lines().collect(); let rows: Vec<_> = gpu.lines().collect();
    need(binding.len() == 3 && rows.len() == 3 && binding[0] == "mode\tair_sha256\tll_sha256\tmetallib_sha256\tinspection_route" &&
         rows[0] == "mode\tloaded_sha256\tstatus\terror\tinputs_unchanged", "receipt schema/coverage")?;
    for (i, mode) in MODES.iter().enumerate() {
        let b: Vec<_> = binding[i + 1].split('\t').collect(); let r: Vec<_> = rows[i + 1].split('\t').collect();
        need(b.len() == 5 && r.len() == 5 && b[0] == *mode && r[0] == *mode, "receipt mode/columns")?;
        need(b[1..4].iter().all(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())) &&
             ["llvm-dis-exact-air", "metal-ir-reader-exact-air"].contains(&b[4]), "compiler hash/inspection binding")?;
        need(r[1] == b[3] && r[2..] == ["4", "0", "1"], "loaded library/status/input binding")?;
    } Ok(())
}
fn validate(out: &Path) -> Result<()> {
    receipts(&text(&out.join("bindings.tsv"))?, &text(&out.join("gpu.tsv"))?)?;
    let metadata = text(&out.join("metadata.tsv"))?;
    need(metadata == "runner\tmacos-26\nos_build\t25G332\narchitecture\tarm64\n" ||
         metadata == "runner\txcode-27\nos_build\t26A434\narchitecture\tarm64\n", "reference cohort metadata")?;
    let mut failures = Vec::new();
    let mut measured = String::from("mode\tcase\tpixel\tp_bits\tn_bits\tp_basis1_bits\tp_basis2_bits\tn_basis1_bits\tn_basis2_bits\tideal_p_bits\tideal_n_bits\tideal_p_basis1_bits\tideal_p_basis2_bits\tideal_n_basis1_bits\tideal_n_basis2_bits\tdelta_p_ulp\tdelta_n_ulp\tdelta_p_basis1_ulp\tdelta_p_basis2_ulp\tdelta_n_basis1_ulp\tdelta_n_basis2_ulp\tmeasured_basis_model_bits\tmeasured_model_delta_ulp\n");
    let mut metrics = String::from("mode\tcase\tchannel\tfinite_positive_pixels\tmax_abs_ulp\texact_pixels\trequired_max_ulp\tcase_result\n");
    let mut integers = String::from("mode\trow\thalf_bits\tu16_actual\tu16_expected\n");
    for (m, mode) in MODES.iter().enumerate() {
        air(&text(&out.join(format!("{mode}.ll")))?)?;
        let raw = read(&out.join(format!("{mode}-u16.bin")))?;
        let result = conversion(&raw, m as u32); println!("{mode} conversion: {result:?}");
        if result.is_err() { failures.push(format!("{mode}: conversion")); }
        if raw.len() == CONVERSION_BYTES { for (i, &(h, v)) in CASES.iter().enumerate() {
            let p = 64 + i * 12; let got = u16::from_le_bytes([raw[p], raw[p + 1]]);
            integers += &format!("{mode}\t{i}\t{h:04x}\t{got:04x}\t{v:04x}\n");
        }}
        for case in 0..2 {
            let a = read(&out.join(format!("{mode}-v{case}-0.bin")))?; let b = read(&out.join(format!("{mode}-v{case}-1.bin")))?;
            let result = varying(&a, &b, m as u32, case); println!("{mode} varying {case}: {result:?}");
            if result.is_err() { failures.push(format!("{mode}: varying {case}")); }
            let mut maximum = [0u32; 6]; let mut exact = [0u32; 6]; let mut finite = [0u32; 6];
            if a.len() == IMAGE_BYTES && b.len() == IMAGE_BYTES { for i in 0..16 {
                let v = observed(&a, &b, i); let e = ideal(case, i);
                measured += &format!("{mode}\t{case}\t{i}");
                for bits in v.into_iter().chain(e) { measured += &format!("\t{bits:08x}"); }
                for c in 0..6 {
                    let good = f32::from_bits(v[c]).is_finite() && v[c] >> 31 == 0;
                    if good {
                        measured += &format!("\t{}", i64::from(v[c]) - i64::from(e[c]));
                        finite[c] += 1; maximum[c] = maximum[c].max(v[c].abs_diff(e[c])); exact[c] += u32::from(v[c] == e[c]);
                    } else { measured += "\tNA"; }
                }
                let model = measured_weights_model(case,v);
                let delta = if f32::from_bits(model).is_finite() && model >> 31 == 0 && f32::from_bits(v[0]).is_finite() && v[0] >> 31 == 0 {
                    (i64::from(v[0]) - i64::from(model)).to_string()
                } else { "NA".into() };
                measured += &format!("\t{model:08x}\t{delta}\n");
            }}
            for (c, name) in ["large_p", "large_n", "p_basis1", "p_basis2", "n_basis1", "n_basis2"].iter().enumerate() {
                let max = if finite[c] == 16 { maximum[c].to_string() } else { "NA".into() };
                let tolerance = if c >= 4 || (case == 0 && c < 2) { 0 } else { 4 };
                metrics += &format!("{mode}\t{case}\t{name}\t{}\t{max}\t{}\t{tolerance}\t{}\n", finite[c], exact[c], if result.is_ok() { "pass" } else { "fail" });
            }
        }
    }
    save(&out.join("measured-u16.tsv"), integers.as_bytes())?; save(&out.join("measured-varying.tsv"), measured.as_bytes())?;
    save(&out.join("metrics.tsv"), metrics.as_bytes())?;
    need(failures.is_empty(), &failures.join("; "))?;
    println!("RAW_AND_AIR_TEXT_GATES_PASS; native provenance additionally requires run.sh compiler/runtime/hash chain"); Ok(())
}
fn main() {
    let a: Vec<_> = env::args().collect();
    let result = if a.len() != 3 { Err("usage: check generate|air|validate PATH".into()) } else {
        let path = Path::new(&a[2]);
        match a[1].as_str() {
            "generate" => generate(path), "air" => text(path).and_then(|s| air(&s)).map(|proof| print!("{proof}")),
            "validate" => validate(path), _ => Err("unknown operation".into()),
        }
    };
    if let Err(e) = result { eprintln!("UNQUALIFIED: {e}"); std::process::exit(3); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> String {
        ["define void @scalar_half_ushort() {", "%x = load half, ptr addrspace(1) %input",
         "%v = tail call i16 @air.convert.u.i16.f.f16(half %x)", "store i16 %v, ptr addrspace(1) %out", "ret void", "}",
         "declare i16 @air.convert.u.i16.f.f16(half)", "define void @large_vertex() {", "ret void", "}",
         "define void @large_fragment() {", "ret void", "}", "!air.vertex = !{!0}", "!air.fragment = !{!1}",
         "!0 = !{ptr @large_vertex, !2}", "!1 = !{ptr @large_fragment, !3}", "!2 = !{!4, !5}", "!3 = !{!6, !7}",
         "!4 = !{!\"air.vertex_output\", !\"user(locn0)\", !\"float3\"}",
         "!5 = !{!\"air.vertex_output\", !\"user(locn1)\", !\"float3\"}",
         "!6 = !{!\"air.fragment_input\", !\"user(locn0)\", !\"float3\", !\"air.center\", !\"air.perspective\"}",
         "!7 = !{!\"air.fragment_input\", !\"user(locn1)\", !\"float3\", !\"air.center\", !\"air.no_perspective\"}"].join("\n")
    }
    #[test] fn live_call_and_link_graph() { assert!(air(&fixture()).is_ok()); }
    #[test] fn generated_linkage_identity_is_exact_and_unambiguous() {
        let generated = fixture().replace("user(locn0)", "generated(3fooDv3_f)")
            .replace("user(locn1)", "generated(3barDv3_f)");
        assert!(air(&generated).is_ok());
        for bad in [
            generated.replace("generated(3barDv3_f)", "generated(3fooDv3_f)"),
            generated.replacen("generated(3fooDv3_f)", "user(3fooDv3_f)", 1),
            generated.replace("generated(3fooDv3_f)", "generated()"),
            generated.replace("generated(3fooDv3_f)", "generated(3fooDv3_f"),
            generated.replace("generated(3fooDv3_f)", "generated(3foo Dv3_f)"),
            generated.replace("!\"generated(3fooDv3_f)\"", "!\"generated(3fooDv3_f)\", !\"user(extra)\""),
        ] { assert!(air(&bad).is_err()); }
    }
    #[test] fn refuses_conversion_substitutions_dead_and_comment_only() {
        for (from, to) in [("u.i16.f.f16", "s.i16.f.f16"), ("call i16", "call i32"), ("half %x)", "float %x)"),
            ("half %x)", "half 0xH3C00)"), ("store i16 %v", "store i16 0"), ("store i16 %v", "; store i16 %v"),
            ("%x = load half", "%x = fpext half"), ("declare i16", "declare i32"), ("(half)\n", "(half, half)\n")] {
            assert!(air(&fixture().replace(from, to)).is_err(), "{from} -> {to}");
        }
        assert!(air("declare i16 @air.convert.u.i16.f.f16(half)\n").is_err());
    }
    #[test] fn refuses_link_type_qualifier_and_metadata_alias_errors() {
        for (from, to) in [("float3", "half3"), ("air.no_perspective", "air.perspective"),
            ("air.center", "air.centroid"), ("!3 = !{!6, !7}", "!3 = !{!6, !6}"),
            ("!air.fragment = !{!1}", "!air.fragment = !{!0}")] {
            assert!(air(&fixture().replace(from, to)).is_err());
        }
    }
    #[test] fn receipt_binding_negatives() {
        let h = "a".repeat(64);
        let b = format!("mode\tair_sha256\tll_sha256\tmetallib_sha256\tinspection_route\nprecise\t{h}\t{h}\t{h}\tllvm-dis-exact-air\nfast\t{h}\t{h}\t{h}\tmetal-ir-reader-exact-air\n");
        let r = format!("mode\tloaded_sha256\tstatus\terror\tinputs_unchanged\nprecise\t{h}\t4\t0\t1\nfast\t{h}\t4\t0\t1\n");
        assert!(receipts(&b, &r).is_ok());
        for bad in [r.replace("\t4\t0\t1", "\t3\t0\t1"), r.replace("\t4\t0\t1", "\t4\t1\t1"),
                    r.replace("\t4\t0\t1", "\t4\t0\t0"), r.replace(&h, &"b".repeat(64)), r.replace("fast", "precise")] {
            assert!(receipts(&b, &bad).is_err());
        }
    }
}
