//! Generic native observation of ordinary fast/precise binary32 atan2 and log2.
//! Inputs and bounds are generated here; the GPU kernel receives operands only.
//! The f64 host reference only measures documented error bounds,
//! never a substitute result. Numeric bound findings are recorded, not gated.
use std::{env, fmt::Write as _, fs, path::Path, process::ExitCode};

const CATEGORIES: [&str; 12] = [
    "special_pair", "log2_special", "log2_power_of_two", "log2_power_neighbor",
    "log2_unit_interval", "log2_near_one", "log2_random", "atan2_circle",
    "atan2_circle_scaled", "atan2_axis_near", "atan2_ratio_extreme", "atan2_random",
];
const SPECIALS: [u32; 18] = [
    0x0000_0000, 0x8000_0000, 0x0000_0001, 0x8000_0001, 0x007f_ffff, 0x807f_ffff,
    0x0080_0000, 0x8080_0000, 0x3f80_0000, 0xbf80_0000, 0x7f7f_ffff, 0xff7f_ffff,
    0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0xffc5_4321, 0x7f80_0001, 0x7fc1_2345,
];
const ONE: u32 = 0x3f80_0000;
const STAMP: u32 = 0x3d1f_6a2b;
const INPUT: usize = 16;
const SAMPLE: usize = 32;
const PROFILES: [&str; 2] = ["legacy", "modern"];

#[derive(Clone, Copy)]
struct Rec { a: u32, b: u32, tag: u32 }

fn corpus() -> Vec<Rec> {
    let mut v = Vec::new();
    let mut push = |a: u32, b: u32, cat: &str| {
        let tag = u32::try_from(CATEGORIES.iter().position(|c| *c == cat).expect("category")).expect("tag");
        v.push(Rec { a, b, tag });
    };
    for &y in &SPECIALS { for &x in &SPECIALS { push(y, x, "special_pair"); } }
    for a in [0xbf00_0000, 0xc180_0000, 0x0040_0000, 0x0000_0100, 0x007f_fffe, 0x0080_0001, 0x7f7f_fffe] {
        push(a, ONE, "log2_special");
    }
    for e in 1_u32..=254 {
        let p = e << 23;
        push(p, ONE, "log2_power_of_two");
        push(p - 1, ONE, "log2_power_neighbor");
        push(p + 1, ONE, "log2_power_neighbor");
    }
    for i in 0..=1024_u32 { push(0x3f00_0000 + i * 0x4000, ONE, "log2_unit_interval"); }
    for k in 1..=32_u32 { push(ONE - k, ONE, "log2_near_one"); push(ONE + k, ONE, "log2_near_one"); }
    let mut s = 0x9e37_79b9_u32;
    let mut next = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; s };
    for _ in 0..384 { let r = next(); push(0x0080_0000 + r % 0x7f00_0000, ONE, "log2_random"); }
    for i in 0..1024_u32 {
        let t = -std::f64::consts::PI + std::f64::consts::TAU * (f64::from(i) + 0.5) / 1024.0;
        let (y, x) = (t.sin(), t.cos());
        push(f32b(y), f32b(x), "atan2_circle");
        for scale in [2f64.powi(-100), 2f64.powi(100)] {
            push(f32b(y * scale), f32b(x * scale), "atan2_circle_scaled");
        }
    }
    for t in [2f64.powi(-20), 2f64.powi(-60), 2f64.powi(-125)] {
        for (sy, sx) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            push(f32b(sy * t), f32b(sx), "atan2_axis_near");
            push(f32b(sy), f32b(sx * t), "atan2_axis_near");
        }
    }
    for (y, x) in [(100, -100), (-100, 100), (126, -126), (-126, 126), (60, -60), (-60, 60)] {
        for (sy, sx) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            push(f32b(sy * 2f64.powi(y)), f32b(sx * 2f64.powi(x)), "atan2_ratio_extreme");
        }
    }
    for _ in 0..512 {
        let r = next();
        let ey = i32::try_from(r % 81).expect("exp") - 40;
        let ex = i32::try_from((r >> 8) % 81).expect("exp") - 40;
        let my = 1.0 + f64::from(next() >> 8) / 16_777_216.0;
        let mx = 1.0 + f64::from(next() >> 8) / 16_777_216.0;
        let sy = if r & 1 == 0 { 1.0 } else { -1.0 };
        let sx = if r & 2 == 0 { 1.0 } else { -1.0 };
        push(f32b(sy * my * 2f64.powi(ey)), f32b(sx * mx * 2f64.powi(ex)), "atan2_random");
    }
    v
}
#[expect(clippy::cast_possible_truncation, reason = "explicit binary64-to-binary32 rounding")]
fn f32b(x: f64) -> u32 { (x as f32).to_bits() }
fn f(b: u32) -> f64 { f64::from(f32::from_bits(b)) }
fn normal(b: u32) -> bool { f32::from_bits(b).is_normal() }
/// Spacing of binary32 at |value|, with the subnormal spacing as floor.
fn ulp(value: f64) -> f64 {
    #[expect(clippy::cast_possible_truncation, reason = "binary32 magnitude class")]
    let m = (value.abs() as f32).to_bits() & 0x7fff_ffff;
    let e = i32::try_from(m >> 23).expect("exponent").max(1) - 127;
    2f64.powi(e - 23)
}
fn class(b: u32) -> &'static str {
    let v = f32::from_bits(b);
    let neg = b >> 31 == 1;
    if v.is_nan() { "nan" } else if v.is_infinite() { if neg { "-inf" } else { "+inf" } }
    else if v == 0.0 { if neg { "-0" } else { "+0" } }
    else if v.is_subnormal() { if neg { "-subnormal" } else { "+subnormal" } }
    else if neg { "-normal" } else { "+normal" }
}

fn encode(v: &[Rec]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * INPUT);
    for r in v { for w in [r.a, r.b, r.tag, 0] { out.extend_from_slice(&w.to_le_bytes()); } }
    out
}
fn words(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Fast atan2 is specified only as atan(y/x) (+/-pi), undefined if x or y is
/// zero; this derives a composed bound for normal operands with a normal quotient.
fn atan2_bound(y: u32, x: u32) -> Option<(f64, f64)> {
    if !normal(y) || !normal(x) { return None; }
    let q = f(y) / f(x);
    if !(2f64.powi(-126)..=2f64.powi(126)).contains(&q.abs()) { return None; }
    let r = f(y).atan2(f(x));
    let base = 5.0 * ulp(q.atan()) + 1.25 * 2f64.powi(-23);
    Some((r, if f(x) < 0.0 { base + 2f64.powi(-21) } else { base }))
}
/// Fast log2: absolute 2^-22 on [0.5, 2], otherwise 2 ulp for positive x.
/// Subnormal x may be flushed (MSL 8.5), so only positive normals are bounded.
fn log2_bound(a: u32) -> Option<(f64, f64)> {
    if !normal(a) || a >> 31 == 1 { return None; }
    let r = f(a).log2();
    Some((r, if (0.5..=2.0).contains(&f(a)) { 2f64.powi(-22) } else { 2.0 * ulp(r) }))
}
fn precise_bound(r: f64, ulps: f64) -> f64 { ulps * ulp(r) }

struct Checked { integrity: Vec<String>, json: String, violations: Vec<(usize, usize)> }

fn check(input: &[u8], uploaded: &[u8], results: &[u8], perturb: &[(usize, usize)]) -> Checked {
    let mut integrity = Vec::new();
    if uploaded != input { integrity.push("uploaded_inputs_differ".into()); }
    let recs = words(input);
    let n = recs.len() / 4;
    if results.len() != n * SAMPLE { integrity.push("result_size".into()); }
    let res = words(results);
    let mut violations = Vec::new();
    let mut exact = [0_u32; 4];
    let mut bounded = [0_u32; 4];
    let mut max_err = [0f64; 4];
    let mut specials = String::new();
    let mut pow2 = (0_u32, 0_u32, 0_u32, 0_u32);
    let mut fast_eq_precise = [0_u32; 2];
    for i in 0..n.min(res.len() / 8) {
        let (a, b, tag) = (recs[4 * i], recs[4 * i + 1], recs[4 * i + 2]);
        let s = &res[8 * i..8 * i + 8];
        let idx = u32::try_from(i).expect("index");
        if s[0] != a || s[1] != b || s[2] != idx || s[3] != STAMP ^ a ^ b ^ tag ^ idx {
            integrity.push(format!("identity:{i}"));
            continue;
        }
        let out = [s[4], s[5], s[6], s[7]];
        fast_eq_precise[0] += u32::from(out[0] == out[1]);
        fast_eq_precise[1] += u32::from(out[2] == out[3]);
        let bounds = [
            atan2_bound(a, b),
            (normal(a) && normal(b)).then(|| { let r = f(a).atan2(f(b)); (r, precise_bound(r, 6.0)) }),
            log2_bound(a),
            (normal(a) && a >> 31 == 0).then(|| { let r = f(a).log2(); (r, precise_bound(r, 4.0)) }),
        ];
        for (op, bound) in bounds.iter().enumerate() {
            let Some((mut r, limit)) = *bound else { continue };
            if perturb.contains(&(i, op)) { r += 2f64.powi(-10); }
            bounded[op] += 1;
            let v = f(out[op]);
            exact[op] += u32::from(out[op] == f32b(r));
            let err = (v - r).abs();
            let scaled = if err.is_nan() { f64::INFINITY } else { err / limit };
            if scaled > max_err[op] { max_err[op] = scaled; }
            if err.is_nan() || err > limit { violations.push((i, op)); }
        }
        let cat = CATEGORIES[usize::try_from(tag).expect("tag")];
        if cat == "log2_power_of_two" {
            let k = f(a).log2();
            pow2.0 += 1;
            pow2.1 += u32::from(f(out[2]) != k);
            pow2.2 += u32::from(f(out[2]).floor() != k);
        }
        if cat == "log2_power_neighbor" {
            // Compare with floor of the correctly rounded value, not of the real
            // logarithm: rounding alone already lands many 2^k-1ulp results on k.
            pow2.3 += u32::from(f(out[2]).floor() != f(f32b(f(a).log2())).floor());
        }
        if cat == "special_pair" || cat == "log2_special" || cat == "atan2_axis_near" || cat == "atan2_ratio_extreme" {
            let _ = write!(specials, "{}{{\"index\":{i},\"category\":\"{cat}\",\"y\":\"{a:08x}\",\"y_class\":\"{}\",\"x\":\"{b:08x}\",\"x_class\":\"{}\",\"fast_atan2\":\"{:08x}\",\"precise_atan2\":\"{:08x}\",\"fast_log2_y\":\"{:08x}\",\"precise_log2_y\":\"{:08x}\",\"fast_atan2_class\":\"{}\",\"fast_log2_class\":\"{}\"}}",
                if specials.is_empty() { "" } else { ",\n  " }, class(a), class(b), out[0], out[1], out[2], out[3], class(out[0]), class(out[2]));
        }
    }
    let names = ["fast_atan2", "precise_atan2", "fast_log2", "precise_log2"];
    let mut ops = String::new();
    for op in 0..4 {
        let count = violations.iter().filter(|v| v.1 == op).count();
        let first: Vec<String> = violations.iter().filter(|v| v.1 == op).take(16).map(|v| v.0.to_string()).collect();
        let maximum = if max_err[op].is_finite() { format!("{:.6}", max_err[op]) } else { "null".into() };
        let _ = write!(ops, "{}\"{}\":{{\"bounded_records\":{},\"exact_correctly_rounded\":{},\"bound_violations\":{count},\"first_violation_indices\":[{}],\"max_error_over_bound\":{maximum},\"nonfinite_error_observed\":{}}}",
            if op == 0 { "" } else { "," }, names[op], bounded[op], exact[op], first.join(","), !max_err[op].is_finite());
    }
    integrity.sort();
    integrity.dedup();
    let json = format!(
        "{{\"schema\":1,\"records\":{n},\"integrity_errors\":[{}],\"bounds\":{{{ops}}},\
         \"bound_basis\":\"MSL Table 8.2 fast log2; atan2 composed from fast atan(5ulp)+division(2.5ulp)+pi addition; precise Table 8.1 (6/4 ulp); bounds recorded, not gated\",\
         \"fast_equals_precise\":{{\"atan2\":{},\"log2\":{}}},\
         \"log2_powers_of_two\":{{\"records\":{},\"not_exact_integer\":{},\"floor_not_k\":{},\"neighbor_floor_differs_from_rounded\":{}}},\
         \"special_records\":[\n  {specials}\n]}}\n",
        integrity.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join(","),
        fast_eq_precise[0], fast_eq_precise[1], pow2.0, pow2.1, pow2.2, pow2.3);
    Checked { integrity, json, violations }
}

/// One deliberately wrong reference per fast operation, chosen from fixed inputs.
fn control_targets(v: &[Rec]) -> Vec<(usize, usize)> {
    let atan = v.iter().position(|r| r.tag == 7 && atan2_bound(r.a, r.b).is_some()).expect("atan2 control");
    let log = v.iter().position(|r| r.a == 0x4080_0000 && r.tag == 2).expect("log2 control");
    let mut t = vec![(atan, 0), (log, 2)];
    t.sort_unstable();
    t
}

/// Exact ABI facts from textual AIR emitted with the same flags.
fn abi(text: &str, profile: &str) -> (bool, String) {
    let lines: Vec<&str> = text.lines().collect();
    let mut failures = Vec::new();
    let mut need = |ok: bool, what: &str| if !ok { failures.push(what.to_string()); };
    let decl = |sig: &str| lines.iter().any(|l| l.starts_with(sig));
    need(decl("declare float @air.fast_atan2.f32(float, float)"), "fast_atan2_f32_declaration");
    need(decl("declare float @air.fast_log2.f32(float)"), "fast_log2_f32_declaration");
    let calls = |name: &str| lines.iter().enumerate().filter(|(_, l)| l.contains(&format!("@{name}(")) && l.contains("call ")).map(|(i, _)| i).collect::<Vec<_>>();
    let atan_calls = calls("air.fast_atan2.f32");
    let log_calls = calls("air.fast_log2.f32");
    need(!atan_calls.is_empty() && atan_calls.iter().all(|&i| lines[i].contains("call fast float @air.fast_atan2.f32(float ")), "fast_atan2_fast_call");
    need(!log_calls.is_empty() && log_calls.iter().all(|&i| lines[i].contains("call fast float @air.fast_log2.f32(float ")), "fast_log2_fast_call");
    need(text.contains("!{!\"air.compile.denorms_disable\"}"), "denorms_disable");
    need(text.contains("!{!\"air.compile.fast_math_enable\"}"), "fast_math_enable");
    if profile == "legacy" { need(text.contains("!{!\"Metal\", i32 2, i32 2, i32 0}"), "metal_2_2_language"); }
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"")
        .replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t");
    let list = |it: Vec<String>| it.iter().map(|s| format!("\"{}\"", esc(s))).collect::<Vec<_>>().join(",\n   ");
    let context = |idx: &[usize]| idx.iter().map(|&i| lines[i.saturating_sub(10)..=i].join("\n")).collect::<Vec<_>>();
    let math_decls = lines.iter().filter(|l| l.starts_with("declare") && (l.contains("atan") || l.contains("log2"))).map(|s| (*s).to_string()).collect();
    let meta = lines.iter().filter(|l| l.starts_with("target triple") || l.starts_with("attributes #") || l.contains("air.compile.") || l.starts_with("!air.version") || l.starts_with("!air.language_version") || l.contains("!\"Metal\"") || l.starts_with("!0 = !{i32")).map(|s| (*s).to_string()).collect();
    let ok = failures.is_empty();
    let json = format!("{{\"profile\":\"{profile}\",\"pass\":{ok},\"failures\":[{}],\n \"math_declarations\":[\n   {}],\n \"fast_atan2_call_context\":[\n   {}],\n \"fast_log2_call_context\":[\n   {}],\n \"profile_metadata\":[\n   {}]}}\n",
        list(failures), list(math_decls), list(context(&atan_calls)), list(context(&log_calls)), list(meta));
    (ok, json)
}

fn read(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    fs::read(dir.join(name)).map_err(|e| format!("{name}: {}", e.kind()))
}
fn write(dir: &Path, name: &str, data: &[u8]) -> Result<(), String> {
    fs::write(dir.join(name), data).map_err(|e| format!("{name}: {}", e.kind()))
}
fn corpus_json(v: &[Rec]) -> String {
    let mut s = String::from("{\"schema\":1,\"layout\":\"le u32 y,x,tag,reserved=0; sample le u32 y,x,index,stamp,fast_atan2,precise_atan2,fast_log2(y),precise_log2(y)\",\"categories\":[");
    s += &CATEGORIES.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
    s += "],\"records\":[\n";
    s += &v.iter().map(|r| format!("[\"{:08x}\",\"{:08x}\",{}]", r.a, r.b, r.tag)).collect::<Vec<_>>().join(",\n");
    s + "\n]}\n"
}

/// Offline self-test: synthetic results equal to rounded references.
fn selftest() -> Result<(), String> {
    let v = corpus();
    if v.len() > 8192 { return Err("corpus too large".into()); }
    let input = encode(&v);
    let mut out = Vec::new();
    for (i, r) in v.iter().enumerate() {
        let idx = u32::try_from(i).expect("index");
        let at = f32b(f(r.a).atan2(f(r.b)));
        let lg = f32b(f(r.a).log2());
        for w in [r.a, r.b, idx, STAMP ^ r.a ^ r.b ^ r.tag ^ idx, at, at, lg, lg] { out.extend_from_slice(&w.to_le_bytes()); }
    }
    let good = check(&input, &input, &out, &[]);
    if !good.integrity.is_empty() || !good.violations.is_empty() { return Err(format!("selftest clean: {:?} {:?}", good.integrity, &good.violations[..good.violations.len().min(4)])); }
    let targets = control_targets(&v);
    let bad = check(&input, &input, &out, &targets);
    if bad.violations != targets { return Err("selftest control".into()); }
    let mut broken = out.clone();
    broken[8] ^= 1;
    if check(&input, &input, &broken, &[]).integrity.is_empty() { return Err("selftest identity".into()); }
    println!("selftest: {} records, control and identity detection pass", v.len());
    Ok(())
}

fn run(args: &[String]) -> Result<u8, String> {
    let arg = |i: usize| args.get(i).map(String::as_str).ok_or_else(|| "usage".to_string());
    match arg(1)? {
        "selftest" => selftest().map(|()| 0),
        "generate" => {
            let dir = Path::new(arg(2)?);
            let v = corpus();
            write(dir, "inputs.bin", &encode(&v))?;
            write(dir, "corpus.json", corpus_json(&v).as_bytes())?;
            Ok(0)
        }
        "abi" => {
            let profile = arg(3)?;
            if !PROFILES.contains(&profile) { return Err("profile".into()); }
            let dir = Path::new(arg(2)?);
            let text = String::from_utf8(read(dir, &format!("{profile}.ll"))?).map_err(|_| "utf8".to_string())?;
            let (ok, json) = abi(&text, profile);
            write(dir, &format!("{profile}-abi.json"), json.as_bytes())?;
            Ok(u8::from(!ok))
        }
        "check" | "wrong-expected" => {
            let dir = Path::new(arg(2)?);
            let profile = arg(3)?;
            if !PROFILES.contains(&profile) { return Err("profile".into()); }
            let input = read(dir, "inputs.bin")?;
            let uploaded = read(dir, &format!("{profile}-uploaded-inputs.bin"))?;
            let results = read(dir, &format!("{profile}-results.bin"))?;
            if arg(1)? == "check" {
                let c = check(&input, &uploaded, &results, &[]);
                let other = if profile == "legacy" { "modern" } else { "legacy" };
                let mut json = c.json;
                if let Ok(o) = read(dir, &format!("{other}-results.bin")) {
                    let same = o.len() == results.len() && o == results;
                    json = json.replacen("{\"schema\":1,", &format!("{{\"schema\":1,\"profile\":\"{profile}\",\"bytes_identical_to_{other}\":{same},"), 1);
                } else {
                    json = json.replacen("{\"schema\":1,", &format!("{{\"schema\":1,\"profile\":\"{profile}\","), 1);
                }
                write(dir, &format!("{profile}-check.json"), json.as_bytes())?;
                return Ok(u8::from(!c.integrity.is_empty()));
            }
            let v = corpus();
            let targets = control_targets(&v);
            let natural = check(&input, &uploaded, &results, &[]).violations;
            let c = check(&input, &uploaded, &results, &targets);
            let mut expected = natural.clone();
            for t in &targets { if !expected.contains(t) { expected.push(*t); } }
            expected.sort_unstable();
            let mut got = c.violations.clone();
            got.sort_unstable();
            // A naturally failing target is not evidence that the deliberately
            // wrong reference was detected. Both chosen controls must start clean.
            let detected = targets.iter().all(|t| !natural.contains(t))
                && got == expected && c.integrity.is_empty();
            let json = format!("{{\"profile\":\"{profile}\",\"perturbed\":{:?},\"natural_violations\":{},\"detected_exactly\":{detected}}}\n", targets, natural.len());
            write(dir, &format!("{profile}-wrong-expected.json"), json.replace('(', "[").replace(')', "]").as_bytes())?;
            Ok(if detected { 1 } else { 0 })
        }
        "audit" => {
            let dir = Path::new(arg(2)?);
            for entry in fs::read_dir(dir).map_err(|e| e.kind().to_string())? {
                let entry = entry.map_err(|e| e.kind().to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".json") || name.ends_with(".txt") || name.ends_with(".ll") {
                    let text = fs::read_to_string(entry.path()).map_err(|e| e.kind().to_string())?;
                    for needle in ["/Users/", "/private/", "/var/folders", "/home/", "runner/work"] {
                        if text.contains(needle) { return Err(format!("{name} contains a host path")); }
                    }
                }
            }
            Ok(0)
        }
        _ => Err("usage: selftest | generate DIR | abi DIR PROFILE | check DIR PROFILE | wrong-expected DIR PROFILE | audit DIR".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => { eprintln!("{e}"); ExitCode::from(3) }
    }
}
