//! Owned inputs, framing, API receipts and basic controls ONLY. No sampling model.
#![forbid(unsafe_code)]
use std::{env, fs, io, path::Path, process::Command};
const N: usize = 1220;
const CH: &str = "id\ttexture\tkind\tmode\tpixel\trows\tskip\n";
const NH: &str = "id\ttexture\tkind\tmode\tpixel\trows\tskip\tformat\twidth\theight\tdepth\tmips\tarray\tsamples\tstorage\tusage\tnormalized\tmin\tmag\tmip\ts\tt\tr\tanisotropy\tlod_min_bits\tlod_max_bits\tcommitted\twait_returned\tcommand_status\terror_present\traw_bytes\n";
const TEXTS: &[&str] = &["cases.tsv", "coords.tsv", "rust-build.txt", "preflight.txt", "preflight-status.txt", "host-baseline.txt",
    "source-sha256.txt", "swift-build.txt", "native-stdio.txt", "native-baseline.json", "native.tsv", "native-status.txt",
    "check.tsv", "check.txt", "check-status.txt", "status.txt", "command-exit.tsv", "SHA256SUMS"];
#[derive(Clone, Copy, Debug)]
struct Case { id: usize, tex: usize, kind: usize, mode: usize, pixel: usize, rows: usize, skip: u32 }
impl Case {
    fn size(self) -> usize { 512 + self.rows * 64 }
    fn name(self) -> String { format!("case{:02}.bin", self.id) }
    fn line(self) -> String { format!("{}\t{}\t{}\t{}\t{}\t{}\t{}", self.id, self.tex, self.kind, self.mode, self.pixel, self.rows, self.skip) }
}
fn cases() -> Vec<Case> {
    let mut cs = Vec::new();
    for tex in 0..2 {
        cs.push(Case { id: cs.len(), tex, kind: 0, mode: 8, pixel: 0, rows: 35, skip: u32::MAX });
        // Current pixel/linear/zero first, then old normalized-edge regressions and address control.
        for mode in [7, 1, 0, 3] { cs.push(Case { id: cs.len(), tex, kind: 1, mode, pixel: (mode >> 1) & 1, rows: N, skip: u32::MAX }); }
        // Deliberately feed pixel coordinates to the normalized linear/zero sampler.
        cs.push(Case { id: cs.len(), tex, kind: 2, mode: 5, pixel: 1, rows: N, skip: u32::MAX });
    }
    cs.push(Case { id: 12, tex: 0, kind: 3, mode: 7, pixel: 1, rows: N, skip: 0 });
    cs
}
fn fail(s: impl Into<String>) -> io::Error { io::Error::other(s.into()) }
fn need(ok: bool, s: &str) -> io::Result<()> { if ok { Ok(()) } else { Err(fail(s)) } }
fn white(x: usize, y: usize) -> bool { !((3..=5).contains(&x) && (1..=3).contains(&y) || x == 1 && y == 2) }
fn seed(tex: usize) -> Vec<u8> {
    // Literal, independently owned packed input words, not decoded/filtered expectations.
    const PACKED: [u32; 35] = [
        0x2011fc00, 0x4017fc01, 0xa0080402, 0xc00ff87f, 0x1ff000ff, 0x7ff009ff, 0x8ff3fe00,
        0xaffbfc7f, 0xc02ffcff, 0x201005ff, 0x4011fe00, 0xa007fe01, 0xc00806ff, 0x1ffffbfe,
        0x3fe00a00, 0x47f3fe01, 0xaff802ff, 0xc02bfffe, 0x201fffff, 0x40100400, 0xa001fc01,
        0xbff807fe, 0xcffffbff, 0x3fe00000, 0x47f00801, 0xaff3fc02, 0xc028007f, 0x201bfcff,
        0x00000401, 0x5ff1fc02, 0xbff7fc7f, 0xcff804ff, 0x3feff9ff, 0x47f00200, 0xaff00a01];
    let mut out = Vec::new();
    for i in 0..35 {
        let word = if tex == 0 { if white(i % 7, i / 7) { u32::MAX } else { 0 } } else { PACKED[i] };
        out.extend(word.to_le_bytes());
    }
    out
}
fn adjacent(x: f32, step: i32) -> f32 {
    if step == 0 { return x; }
    if x == 0.0 { return if step < 0 { -f32::from_bits(1) } else { f32::from_bits(1) }; }
    f32::from_bits(if (x > 0.0) == (step > 0) { x.to_bits() + 1 } else { x.to_bits() - 1 })
}
fn coordinates() -> Vec<(u8, [u32; 4])> {
    let mut out = Vec::new();
    let mut add = |group, x: f32, y: f32, dx, dy| {
        // Native coordinate-space neighbors independently; no odd-size equivalence assertion.
        out.push((group, [adjacent(x, dx), adjacent(y, dy), adjacent(x / 7.0, dx), adjacent(y / 5.0, dy)].map(f32::to_bits)));
    };
    for (x, y) in [(0.5, 0.5), (4.5, 2.5), (1.75, 2.5), (-2.0, -2.0)] { add(0, x, y, 0, 0); }
    for y in 0..5 { for x in 0..7 { add(1, x as f32 + 0.5, y as f32 + 0.5, 0, 0); } }
    for y in -4..=24 { for x in -4..=32 { add(2, x as f32 / 4.0, y as f32 / 4.0, 0, 0); } }
    for x in 0..=14 { for d in -1..=1 { add(3, x as f32 / 2.0, 2.5, d, 0); } }
    for y in 0..=10 { for d in -1..=1 { add(3, 3.5, y as f32 / 2.0, 0, d); } }
    for f in [1.0/1024.0, 1.0/512.0, 1.0/256.0, 1.0/16.0, 0.25, 0.5, 0.75, 255.0/256.0, 511.0/512.0, 1023.0/1024.0] {
        for d in -1..=1 { add(4, 1.5 + f, 1.5 + f, d, d); }
    }
    out
}
fn inputs() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = (0..2).map(|t| (format!("seed{t}.bin"), seed(t))).collect();
    let mut table = String::from("row\tgroup\tpixel_x_bits\tpixel_y_bits\tnormalized_x_bits\tnormalized_y_bits\n");
    let mut bytes = Vec::new();
    for (i, (group, values)) in coordinates().iter().enumerate() {
        table += &format!("{i}\t{group}\t{:08x}\t{:08x}\t{:08x}\t{:08x}\n", values[0], values[1], values[2], values[3]);
        for v in values { bytes.extend(v.to_le_bytes()); }
    }
    out.push(("coords.bin".into(), bytes)); out.push(("coords.tsv".into(), table.into_bytes()));
    out.push(("cases.tsv".into(), (CH.to_string() + &cases().iter().map(|c| c.line() + "\n").collect::<String>()).into_bytes()));
    out
}
fn prepare(root: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    for (name, _) in inputs() { need(!root.join(name).exists(), "fresh_input_files_required")?; }
    for (name, bytes) in inputs() { fs::write(root.join(name), bytes)?; }
    println!("OWNED_INPUTS_PREPARED coordinates=1220 cases=13 arithmetic_model=false"); Ok(())
}
fn inventory(root: &Path) -> io::Result<(usize, u64)> {
    need(fs::symlink_metadata(root)?.file_type().is_dir(), "artifact_root_not_plain_directory")?;
    let cs = cases(); let ins = inputs(); let mut count = 0; let mut bytes = 0;
    for e in fs::read_dir(root)? {
        let e = e?; let m = fs::symlink_metadata(e.path())?;
        need(m.file_type().is_file(), "artifact_not_plain_file")?;
        let name = e.file_name(); let name = name.to_str().ok_or_else(|| fail("artifact_name"))?;
        let cap = if TEXTS.contains(&name) { 65536 } else if let Some((_, b)) = ins.iter().find(|(n, _)| n == name) { b.len() }
        else if let Some(c) = cs.iter().find(|c| c.name() == name) { c.size() }
        else if (0..2).any(|t| name == format!("transfer{t}.bin")) { 3584 } else { return Err(fail("artifact_not_allowlisted")); };
        need(m.len() <= cap as u64, "artifact_file_budget")?; count += 1; bytes += m.len();
    }
    need(count <= 40 && bytes <= 4 * 1024 * 1024, "artifact_total_budget")?; Ok((count, bytes))
}
fn word(b: &[u8], offset: usize) -> u32 { u32::from_le_bytes(b[offset..offset+4].try_into().unwrap()) }
fn bits(b: &[u8], row: usize, slot: usize, channel: usize) -> u32 {
    let (offset, width) = [(0, 4), (16, 2), (32, 4), (24, 2)][slot];
    let at = 256 + row * 64 + offset + channel * width;
    if width == 4 { word(b, at) } else { u16::from_le_bytes(b[at..at+2].try_into().unwrap()) as u32 }
}
fn one(slot: usize) -> u32 { if slot % 2 == 0 { 0x3f800000 } else { 0x3c00 } }
fn health(b: &[u8], c: Case, expected_skip: bool) -> io::Result<()> {
    need(b.len() == c.size(), "raw_size")?;
    need(b[..256].iter().chain(&b[b.len()-256..]).all(|v| *v == 0xcd), "buffer_guard")?;
    for row in 0..c.rows {
        let at = 256 + row * 64;
        if row == c.skip as usize {
            need(expected_skip && b[at..at+64].iter().all(|v| *v == 0xcd), "deliberate_skipped_row_not_exact_poison")?; continue;
        }
        let hash = (0..12).fold(2166136261u32, |h, i| (h ^ word(b, at + i*4)).wrapping_mul(16777619)) ^ 0x53414d50;
        need(word(b, at+48) == hash && word(b, at+52) == row as u32 && word(b, at+56) == c.id as u32 &&
             word(b, at+60) == !(row as u32), "payload_tag_or_row_marker")?;
        for slot in 0..4 { for ch in 0..4 {
            let v = bits(b, row, slot, ch);
            let ok = if slot % 2 == 0 { let f = f32::from_bits(v); f.is_finite() && (0.0..=1.0).contains(&f) }
                     else { v <= 0x3c00 || v == 0x8000 };
            need(ok, "nonfinite_poison_or_outside_UNORM_range")?;
        }}
    }
    Ok(())
}
fn receipt() -> String {
    let mut s = NH.to_string();
    for c in cases() {
        let sampler = if c.kind == 0 { vec!["-".to_string(); 10].join("\t") } else {
            let f = c.mode & 1; let a = if c.mode & 4 == 0 { 0 } else { 4 };
            format!("{}\t{f}\t{f}\t0\t{a}\t{a}\t{a}\t1\t0\t0", usize::from(c.mode & 2 == 0))
        };
        s += &format!("{}\t90\t7\t5\t1\t1\t1\t1\t2\t1\t{sampler}\t1\t1\t4\t0\t{}\n", c.line(), c.size());
    }
    s
}
fn exact(root: &Path, name: &str, wanted: &[u8]) -> bool { fs::read(root.join(name)).is_ok_and(|b| b == wanted) }
fn endpoint(b: &[u8], row: usize, value: bool) -> io::Result<()> {
    for slot in 0..4 { for ch in 0..4 {
        need(bits(b, row, slot, ch) == if value { one(slot) } else { 0 }, "basic_endpoint_control")?;
    }} Ok(())
}
fn controls(observed: &[Vec<u8>]) -> io::Result<()> {
    for row in 0..35 { endpoint(&observed[0], row, white(row % 7, row / 7))?; }
    for (id, mode) in [(1, 7), (2, 1), (3, 0), (4, 3)] {
        let b = &observed[id]; endpoint(b, 0, true)?; endpoint(b, 1, false)?;
        for slot in 0..4 { for ch in 0..4 { need(bits(b, 3, slot, ch) == if mode & 4 == 0 { one(slot) } else { 0 }, "far_outside_edge_zero_control")?; } }
    }
    for ch in 0..4 {
        // Known endpoints plus a strictly-between control, NOT a predicted interpolation value.
        let edge = &observed[4]; let near = &observed[3]; let linear = &observed[1]; let old_linear = &observed[2]; let wrong = &observed[5];
        for slot in 0..4 {
            need(bits(edge, 3, slot, ch) == one(slot) && bits(linear, 3, slot, ch) == 0, "wrong_address_not_distinct")?;
            need(bits(near, 2, slot, ch) == 0 && bits(old_linear, 2, slot, ch) > 0 && bits(old_linear, 2, slot, ch) < one(slot), "old_edge_filter_control_not_distinct")?;
            need(bits(linear, 2, slot, ch) > 0 && bits(linear, 2, slot, ch) < one(slot), "pixel_linear_control_not_between_endpoints")?;
            need(bits(linear, 0, slot, ch) == one(slot) && bits(wrong, 0, slot, ch) == 0, "wrong_coordinate_mode_not_distinct")?;
            need(bits(linear, 2, slot, ch) == bits(linear, 568, slot, ch), "duplicate_control_coordinate_differs")?;
        }
        need(bits(linear, 2, 0, ch) == bits(linear, 2, 2, ch) && bits(linear, 2, 1, ch) == bits(linear, 2, 3, ch), "basic_filter_control_binding_disagreement")?;
        need(bits(old_linear, 2, 0, ch) == bits(old_linear, 2, 2, ch) && bits(old_linear, 2, 1, ch) == bits(old_linear, 2, 3, ch), "old_filter_control_binding_disagreement")?;
    }
    // The deliberately skipped row fails strict health; every other payload repeats a clean native case.
    need(health(&observed[12], cases()[12], false).is_err(), "skip_control_not_rejected")?;
    for row in 1..N { let at = 256 + row * 64; need(observed[12][at..at+48] == observed[1][at..at+48], "skip_control_other_payload_changed")?; }
    Ok(())
}
fn check(root: &Path) -> io::Result<()> {
    fs::write(root.join("check-status.txt"), "FAILED\n")?; inventory(root)?;
    for (name, bytes) in inputs() { need(exact(root, &name, &bytes), "owned_input_changed")?; }
    let receipts = exact(root, "native-status.txt", b"CAPTURED_ALL\n") && exact(root, "native.tsv", receipt().as_bytes());
    let mut observed = Vec::new(); let mut good = receipts;
    let mut table = String::from("case\thealth_with_declared_skip\truntime_constexpr_differing_lanes_observation_only\n");
    for c in cases() {
        let b = fs::read(root.join(c.name())).unwrap_or_default(); let ok = health(&b, c, c.id == 12).is_ok(); good &= ok;
        let mut differences = 0;
        if ok { for row in 0..c.rows { if row != c.skip as usize { for ch in 0..4 {
            differences += usize::from(bits(&b, row, 0, ch) != bits(&b, row, 2, ch));
            differences += usize::from(bits(&b, row, 1, ch) != bits(&b, row, 3, ch));
        }}} }
        table += &format!("{}\t{ok}\t{differences}\n", c.id); observed.push(b);
    }
    for tex in 0..2 {
        let seed = seed(tex); let stride = 28; let mut upload = vec![0xcd; 1792];
        for y in 0..5 { upload[256+y*256..256+y*256+stride].copy_from_slice(&seed[y*stride..(y+1)*stride]); }
        let expected = [upload.as_slice(), upload.as_slice()].concat();
        let ok = exact(root, &format!("transfer{tex}.bin"), &expected); good &= ok;
        table += &format!("transfer{tex}\t{ok}\t-\n");
    }
    let basic = if good { controls(&observed) } else { Err(fail("framing_or_API_failure_before_controls")) };
    table += &format!("basic_controls\t{}\tno_arithmetic_model\n", basic.is_ok()); fs::write(root.join("check.tsv"), table)?;
    need(good, "input_roundtrip_framing_or_native_API_receipt_failed_preserve_all_bytes")?; basic?;
    fs::write(root.join("check-status.txt"), "PASS_BASIC_CONTROLS_ONLY\n")?;
    println!("CAPTURE_HEALTH_AND_BASIC_CONTROLS_PASS cases=13 arithmetic_model=false private_comparison_pending=true"); Ok(())
}
fn self_test() -> io::Result<()> {
    let coords = coordinates(); assert_eq!(coords.len(), N); assert_eq!(coords[2].1, coords[568].1);
    assert!(coords.iter().all(|(_, v)| v.iter().all(|x| f32::from_bits(*x).is_finite())));
    assert_eq!((0..5).map(|g| coords.iter().filter(|(k, _)| *k == g).count()).collect::<Vec<_>>(), [4, 35, 1073, 78, 30]);
    let cs = cases(); assert_eq!(cs.len(), 13); assert_eq!(cs.iter().map(|c| c.size()).sum::<usize>() + 2 * 3584, 877_184);
    for c in cs { assert!(health(&vec![0xcd; c.size()], c, true).is_err()); }
    assert!(white(0, 0) && !white(4, 2) && !white(3, 2) && !white(1, 2) && white(2, 2));
    assert!(inputs().iter().all(|(_, b)| b.len() <= 65536));
    println!("HOST_FRAMING_SELF_TEST_PASS no_native_execution=true no_arithmetic_model=true"); Ok(())
}
fn preflight(root: &Path) -> io::Result<()> {
    fs::write(root.join("preflight-status.txt"), "FAILED\n")?;
    fs::write(root.join("status.txt"), "FAILED_OR_INCOMPLETE_NO_QUALIFICATION\n")?;
    fs::write(root.join("command-exit.tsv"), "run_exit\tnative_exit\tcapture_check_exit\n99\t99\t99\n")?;
    need(cfg!(target_os = "macos") && cfg!(target_endian = "little"), "native_capture_requires_little_endian_macOS")?;
    for (key, val) in [("GITHUB_REPOSITORY", "PellaML/r9c4m2q7v6"), ("GITHUB_REF", "refs/heads/research/probe"),
        ("GITHUB_EVENT_NAME", "workflow_dispatch"), ("GITHUB_RUN_ATTEMPT", "1")] { need(env::var(key).as_deref() == Ok(val), "CI_identity_or_attempt")?; }
    let head = env::var("EXPECTED_HEAD").map_err(|_| fail("expected_head_missing"))?;
    need(head.len() == 40 && head.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && env::var("GITHUB_SHA").as_deref() == Ok(head.as_str()), "exact_approved_head")?;
    inventory(root)?; self_test()?;
    let mut report = format!("source_commit={head}\nrunner_label_is_not_build_evidence=true\n");
    for argv in [&["git", "rev-parse", "HEAD"][..], &["sw_vers"], &["uname", "-m"], &["xcrun", "--show-sdk-version"],
                 &["xcodebuild", "-version"], &["swiftc", "--version"], &["rustc", "--version"]] {
        let o = Command::new(argv[0]).args(&argv[1..]).output()?;
        need(o.status.success() && o.stdout.len() + o.stderr.len() <= 8192, "host_metadata_command_failed_or_unbounded")?;
        let stdout = String::from_utf8_lossy(&o.stdout); if argv[0] == "git" { need(stdout.trim() == head, "checkout_commit_mismatch")?; }
        report += &format!("\n[{}]\n{}{}", argv.join(" "), stdout, String::from_utf8_lossy(&o.stderr));
    }
    fs::write(root.join("host-baseline.txt"), report)?; prepare(root)?; fs::write(root.join("preflight-status.txt"), "PASS\n")
}
fn status(root: &Path, codes: &[String]) -> io::Result<()> {
    let ok = codes == ["0", "0", "0"] && exact(root, "preflight-status.txt", b"PASS\n") &&
        exact(root, "check-status.txt", b"PASS_BASIC_CONTROLS_ONLY\n") && exact(root, "native-status.txt", b"CAPTURED_ALL\n") && inventory(root).is_ok();
    fs::write(root.join("status.txt"), if ok { "CAPTURE_COMPLETE_BASIC_CONTROLS_PASS_PRIVATE_COMPARISON_PENDING\n" } else { "FAILED_OR_INCOMPLETE_NO_QUALIFICATION\n" })?;
    need(ok, "capture_failed_or_incomplete")
}
fn main() {
    let a: Vec<String> = env::args().collect();
    let result = match a.get(1).map(String::as_str) {
        Some("--self-test") if a.len() == 2 => self_test(),
        Some("prepare") if a.len() == 3 => prepare(Path::new(&a[2])),
        Some("preflight") if a.len() == 3 => preflight(Path::new(&a[2])),
        Some("check") if a.len() == 3 => check(Path::new(&a[2])),
        Some("status") if a.len() == 6 => status(Path::new(&a[2]), &a[3..]),
        Some("bounds") if a.len() == 3 => inventory(Path::new(&a[2])).and_then(|(files, bytes)| {
            for name in ["status.txt", "command-exit.tsv"] { need(Path::new(&a[2]).join(name).is_file(), "final_or_incomplete_receipt_missing")?; }
            println!("BOUNDED files={files} bytes={bytes} max_bytes_per_job=4194304"); Ok(())
        }),
        _ => Err(fail("usage: sampling-check --self-test | prepare DIR | preflight DIR | check DIR | status DIR RUN_RC NATIVE_RC CHECK_RC | bounds DIR")),
    };
    if let Err(e) = result { eprintln!("{e}"); std::process::exit(3); }
}
