//! Exact row/value oracle and finite acquisition gates, NOT a CPU renderer.
#![forbid(unsafe_code)]
use std::{env, fs, io, path::Path, process::Command};
const HEADER: &str = "id\ttopology\tinstances\theight\tmode\tsubmitted\twidth\tformat\tsamples\tvertices_or_indices\tbase_instance\tcommitted\twait_returned\tcommand_status\terror_present\traw_bytes\n";
const TEXTS: &[&str] = &["rust-build.txt", "preflight.txt", "preflight-status.txt", "host-baseline.txt",
    "source-sha256.txt", "swift-build.txt", "native-stdio.txt", "native-baseline.json", "native.tsv",
    "native-status.txt", "check.tsv", "check.txt", "check-status.txt", "status.txt", "command-exit.tsv", "SHA256SUMS"];
#[derive(Clone, Copy, Debug)]
struct Case { id: usize, topology: usize, n: usize, h: usize, mode: usize }
impl Case {
    fn size(self) -> usize { 512 + 256 * self.h }
    fn name(self) -> String { format!("case{:02}.bin", self.id) }
    fn submitted(self) -> usize { if self.mode == 1 { 64 } else { self.n } }
    fn mismatch(self) -> usize { match self.mode { 0 => 0, 1 => 4 * (self.n - 64), _ => 3 * (self.n - 64) } }
}
fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for topology in 0..3 { for n in [1usize, 64, 65, 108, 128, 4096] {
        for mode in 0..if n > 64 { 3 } else { 1 } {
            out.push(Case { id: out.len(), topology, n, h: (n + 1).next_power_of_two().max(128), mode });
        }
    }}
    out
}
fn error(why: impl Into<String>) -> io::Error { io::Error::other(why.into()) }
fn need(ok: bool, why: &str) -> io::Result<()> { if ok { Ok(()) } else { Err(error(why)) } }
fn exact(root: &Path, name: &str, expected: &str) -> bool {
    fs::read(root.join(name)).is_ok_and(|b| b == expected.as_bytes())
}
// Row y is owned by instance y; every other row must remain the clear sentinel.
// All channels and all row boundaries are exact dyadics, independently of Metal.
fn pixel(c: Case, y: usize, mode: usize) -> [u32; 4] {
    let active = if mode == 1 { c.n.min(64) } else { c.n };
    let values = if y >= active { [-1.0f32, -2.0, -4.0, -8.0] } else {
        let id = if mode == 2 { y % 64 } else { y };
        let v = (id + 1) as f32;
        [v, v / 8192.0, v / 16384.0, 0.5]
    };
    values.map(f32::to_bits)
}
#[derive(Debug)]
struct Delta { words: usize, first: String }
fn compare(raw: &[u8], c: Case, mode: usize) -> io::Result<Delta> {
    need(raw.len() == c.size(), "raw_length")?;
    for (i, &v) in raw.iter().enumerate() {
        let payload = i >= 256 && i < 256 + 256 * c.h && (i - 256) % 256 < 16;
        need(payload || v == 0xcd, "readback_guard_or_padding")?;
    }
    let mut d = Delta { words: 0, first: "none".into() };
    for y in 0..c.h { for (channel, want) in pixel(c, y, mode).into_iter().enumerate() {
        let at = 256 + y * 256 + channel * 4;
        let got = u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
        if got != want {
            if d.words == 0 { d.first = format!("row={y},channel={channel},got={got:08x},want={want:08x}"); }
            d.words += 1;
        }
    }}
    Ok(d)
}
fn receipt() -> String {
    let mut s = HEADER.to_string();
    for c in cases() {
        s += &format!("{}\t{}\t{}\t{}\t{}\t{}\t1\t125\t1\t{}\t0\t1\t1\t4\t0\t{}\n",
            c.id, c.topology, c.n, c.h, c.mode, c.submitted(), if c.topology == 2 { 6 } else { 4 }, c.size());
    }
    s
}
fn inventory(root: &Path) -> io::Result<(usize, u64)> {
    need(fs::symlink_metadata(root)?.file_type().is_dir(), "artifact_root_not_plain_directory")?;
    let cs = cases(); let mut count = 0; let mut bytes = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?; let meta = fs::symlink_metadata(entry.path())?;
        need(meta.file_type().is_file(), "artifact_not_plain_file")?;
        let name = entry.file_name(); let name = name.to_str().ok_or_else(|| error("artifact_name"))?;
        let cap = if TEXTS.contains(&name) { 65536 } else {
            cs.iter().find(|c| c.name() == name).ok_or_else(|| error("artifact_not_allowlisted"))?.size() as u64
        };
        need(meta.len() <= cap, "artifact_file_budget")?;
        count += 1; bytes += meta.len();
    }
    need(count <= 64 && bytes <= 24 * 1024 * 1024, "artifact_total_budget")?;
    Ok((count, bytes))
}
fn check(root: &Path) -> io::Result<()> {
    fs::write(root.join("check-status.txt"), "FAILED\n")?;
    inventory(root)?;
    let receipts = exact(root, "native-status.txt", "CAPTURED_ALL\n") && exact(root, "native.tsv", &receipt());
    let mut pass = receipts;
    let mut table = String::from("id\ttopology\tinstances\theight\tmode\texpected_exact_match\tactual_exact_match\tmismatch_words\texpected_mismatch_words\tcontrol_signature_match\tpass\tfirst_difference_or_error\n");
    for c in cases() {
        let result = fs::read(root.join(c.name())).and_then(|raw| {
            let correct = compare(&raw, c, 0)?;
            let signature = compare(&raw, c, c.mode)?.words == 0;
            Ok((correct, signature))
        });
        match result {
            Ok((d, signature)) => {
                let ok = d.words == c.mismatch() && signature;
                pass &= ok;
                table += &format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", c.id, c.topology,
                    c.n, c.h, c.mode, c.mode == 0, d.words == 0, d.words, c.mismatch(), signature, ok, d.first);
            }
            Err(_) => { pass = false; table += &format!("{}\t{}\t{}\t{}\t{}\t{}\tinvalid\t-\t{}\tfalse\tfalse\tmissing_or_invalid_raw\n",
                c.id, c.topology, c.n, c.h, c.mode, c.mode == 0, c.mismatch()); }
        }
    }
    fs::write(root.join("check.tsv"), table)?;
    println!("exact_check={pass} clean_native_receipts={receipts} positive=18 required_negative_rejections=24 tolerance=0");
    need(pass, "exact_comparison_or_native_receipt_failed; preserve_raw_bytes")?;
    fs::write(root.join("check-status.txt"), "PASS\n")
}
// Host-only oracle tests: in-memory bytes, never emitted as native observations.
fn synthetic(c: Case, mode: usize) -> Vec<u8> {
    let mut b = vec![0xcd; c.size()];
    for y in 0..c.h { for (ch, word) in pixel(c, y, mode).into_iter().enumerate() {
        let at = 256 + y * 256 + ch * 4; b[at..at + 4].copy_from_slice(&word.to_le_bytes());
    }}
    b
}
fn self_test() -> io::Result<()> {
    let cs = cases(); assert_eq!(cs.len(), 42);
    assert_eq!(cs.iter().map(|c| c.size()).sum::<usize>(), 20_272_128);
    assert_eq!(cs.iter().filter(|c| c.mode == 0).count(), 18);
    for c in cs {
        assert!(c.h > c.n && c.h.is_power_of_two());
        let mut b = synthetic(c, c.mode);
        assert_eq!(compare(&b, c, 0)?.words, c.mismatch());
        assert_eq!(compare(&b, c, c.mode)?.words, 0);
        for y in 0..c.n {
            assert_eq!(f32::from_bits(pixel(c, y, 0)[0]), (y + 1) as f32);
            let top = 1.0 - 2.0 * y as f32 / c.h as f32;
            let bottom = 1.0 - 2.0 * (y + 1) as f32 / c.h as f32;
            assert_eq!((1.0 - (top + bottom) / 2.0) * c.h as f32 / 2.0, y as f32 + 0.5);
        }
        for at in [0, 255, 272, c.size() - 1] {
            b[at] ^= 1; assert!(compare(&b, c, 0).is_err()); b[at] ^= 1;
        }
        assert!(compare(&b[..b.len() - 1], c, 0).is_err());
    }
    let c = cases()[0]; let mut b = synthetic(c, 0);
    b[256..260].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
    assert_eq!(compare(&b, c, 0)?.words, 1);
    b = synthetic(c, 0); b[256 + 256 * c.n] ^= 1;
    assert_eq!(compare(&b, c, 0)?.words, 1);
    println!("HOST_ORACLE_TESTS_PASS native_executed=false cases=42 guards_padding_NaN_clear_checked=true");
    Ok(())
}
fn preflight(root: &Path) -> io::Result<()> {
    fs::write(root.join("preflight-status.txt"), "FAILED\n")?;
    fs::write(root.join("status.txt"), "FAILED_OR_INCOMPLETE_NO_NATIVE_QUALIFICATION\n")?;
    fs::write(root.join("command-exit.tsv"), "run_exit\tnative_exit\tcomparison_exit\n99\t99\t99\n")?;
    need(cfg!(target_os = "macos") && cfg!(target_endian = "little"), "native_acquisition_requires_little_endian_macOS")?;
    for (key, value) in [("GITHUB_REPOSITORY", "PellaML/r9c4m2q7v6"), ("GITHUB_REF", "refs/heads/research/probe"),
                         ("GITHUB_EVENT_NAME", "workflow_dispatch"), ("GITHUB_RUN_ATTEMPT", "1")] {
        need(env::var(key).as_deref() == Ok(value), "CI_identity_or_attempt_gate")?;
    }
    let head = env::var("EXPECTED_HEAD").map_err(|_| error("expected_head_missing"))?;
    need(head.len() == 40 && head.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) &&
         env::var("GITHUB_SHA").as_deref() == Ok(head.as_str()), "exact_approved_head_required")?;
    inventory(root)?; self_test()?;
    let mut report = format!("source_commit={head}\nrunner_label_is_not_build_evidence=true\n");
    for argv in [&["git", "rev-parse", "HEAD"][..], &["sw_vers"], &["uname", "-m"],
                 &["xcrun", "--show-sdk-version"], &["xcodebuild", "-version"], &["swiftc", "--version"], &["rustc", "--version"]] {
        let output = Command::new(argv[0]).args(&argv[1..]).output()?;
        need(output.status.success() && output.stdout.len() + output.stderr.len() <= 8192, "host_baseline_command_failed_or_unbounded")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if argv[0] == "git" { need(stdout.trim() == head, "checkout_commit_mismatch")?; }
        report += &format!("\n[{}]\n{}{}", argv.join(" "), stdout, String::from_utf8_lossy(&output.stderr));
    }
    fs::write(root.join("host-baseline.txt"), report)?;
    fs::write(root.join("preflight-status.txt"), "PASS\n")
}
fn status(root: &Path, codes: &[String]) -> io::Result<()> {
    let passed = codes == ["0", "0", "0"] && exact(root, "preflight-status.txt", "PASS\n") &&
        exact(root, "check-status.txt", "PASS\n") && exact(root, "native-status.txt", "CAPTURED_ALL\n") && inventory(root).is_ok();
    fs::write(root.join("status.txt"), if passed { "PASS_EXACT_NATIVE_INSTANCE_OBSERVATIONS\n" } else { "FAILED_OR_INCOMPLETE_NO_NATIVE_QUALIFICATION\n" })?;
    need(passed, "acquisition_failed_or_incomplete")
}
fn main() {
    let a: Vec<String> = env::args().collect();
    let result = match a.get(1).map(String::as_str) {
        Some("--self-test") if a.len() == 2 => self_test(),
        Some("preflight") if a.len() == 3 => preflight(Path::new(&a[2])),
        Some("check") if a.len() == 3 => check(Path::new(&a[2])),
        Some("exact") if a.len() == 4 => a[3].parse::<usize>().ok().and_then(|id| cases().get(id).copied())
            .ok_or_else(|| error("case_id_required_0_to_41")).and_then(|c| {
                let d = compare(&fs::read(Path::new(&a[2]).join(c.name()))?, c, 0)?;
                println!("EXACT_CASE id={} mismatch_words={} first={}", c.id, d.words, d.first);
                need(d.words == 0, "exact_comparison_failed")
            }),
        Some("status") if a.len() == 6 => status(Path::new(&a[2]), &a[3..]),
        Some("bounds") if a.len() == 3 => inventory(Path::new(&a[2])).and_then(|(files, bytes)| {
            for name in ["status.txt", "command-exit.tsv"] {
                need(Path::new(&a[2]).join(name).is_file(), "final_receipt_missing")?;
            }
            println!("BOUNDED files={files} bytes={bytes} max_bytes=25165824"); Ok(())
        }),
        _ => Err(error("usage: instancing-check --self-test | preflight DIR | check DIR | exact DIR ID | status DIR RUN_RC NATIVE_RC CHECK_RC | bounds DIR")),
    };
    if let Err(e) = result { eprintln!("{e}"); std::process::exit(3); }
}
