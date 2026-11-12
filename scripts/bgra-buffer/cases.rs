// Owned synthetic BGRA buffer storage probe. No dependencies or CPU fallback.
// Source binding, SHA-256 and bounded artifact guards reuse the public harness.
#![forbid(unsafe_code)]
use std::{collections::BTreeMap, env, fs, io::Write, path::Path, process::Command};
type Result<T> = std::result::Result<T, String>;
const SOURCE: [&str; 5] = [
    ".github/workflows/bgra-buffer.yml",
    "scripts/bgra-buffer/cases.rs",
    "scripts/bgra-buffer/probe.metal",
    "scripts/bgra-buffer/probe.swift",
    "scripts/bgra-buffer/run.sh",
];
const PARENT: usize = 65536;
const G: usize = 64;
const STRIDE: usize = PARENT + G * 2;
const CAPTURES: usize = 13;
const POISON: u8 = 0xcd;
const LIMIT: u64 = 2 * 1024 * 1024;
const COMPLETE: &str = "schema=1\ncommands=1\ncaptures=13\nclears=6\ndraws=6\nskipped_draws=1\nblit_copies=26\nsampling=0\ncompute=0\n";
const EXIT_OK: &str = "{\"exit\":0,\"phase\":\"complete\"}\n";
const NEGATIVE: &str = "{\"wrong_expected_rejected\":true,\"native_bytes_unchanged\":true,\"capture\":0,\"byte\":0,\"exit\":2}\n";
const HEADER: &str = "capture\tcase\top\twidth\theight\tpitch\toffset\tview_bytes\tparent_bytes\tformat\tusage\tstorage\tparent_storage\tparent_identity\ttype\tdepth\tmips\tslices\tsamples\thazard\tparent_hazard\tidentity_swizzle\ttexture_parent\tframebuffer_only\talignment\tload\tstore\tdraws\tvertices\tpso_bound\tblending\twrite_mask\trasterization\talpha_coverage\talpha_one\tstatus\terror";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    width: usize,
    height: usize,
    pitch: usize,
    offset: usize,
}
const LAYOUTS: [Layout; 6] = [
    Layout {
        width: 1,
        height: 1,
        pitch: 256,
        offset: 0,
    },
    Layout {
        width: 5,
        height: 3,
        pitch: 256,
        offset: 0,
    },
    Layout {
        width: 5,
        height: 3,
        pitch: 256,
        offset: 256,
    },
    Layout {
        width: 5,
        height: 3,
        pitch: 512,
        offset: 256,
    },
    Layout {
        width: 104,
        height: 104,
        pitch: 512,
        offset: 0,
    },
    Layout {
        width: 104,
        height: 104,
        pitch: 512,
        offset: 256,
    },
];
fn need(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn layout_valid(c: Layout, view_bytes: usize) -> Result<()> {
    need(c.width > 0 && c.height > 0, "nonempty layout")?;
    let row = c.width.checked_mul(4).ok_or("row overflow")?;
    let span = c.pitch.checked_mul(c.height).ok_or("view overflow")?;
    let end = c.offset.checked_add(span).ok_or("parent overflow")?;
    need(
        row <= c.pitch
            && c.pitch % 256 == 0
            && c.offset % 256 == 0
            && span == view_bytes
            && end <= PARENT,
        "aligned pitch*height parent identity",
    )
}
fn plan() -> String {
    let layouts = LAYOUTS
        .iter()
        .map(|c| {
            format!(
                "{{\"width\":{},\"height\":{},\"pitch\":{},\"offset\":{}}}",
                c.width, c.height, c.pitch, c.offset
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("{{\"schema\":1,\"parent\":65536,\"guard_bytes\":64,\"captures\":13,\"clear_rgba\":[17,83,149,231],\"layouts\":[{layouts}]}}\n")
}
fn initial() -> Vec<u8> {
    (0..PARENT)
        .map(|i| ((i * 73) ^ ((i >> 8) * 19) ^ 0xa5) as u8)
        .collect()
}
fn guarded(data: &[u8]) -> Vec<u8> {
    [vec![POISON; G], data.to_vec(), vec![POISON; G]].concat()
}
fn bgra(case: usize, draw: bool, x: usize, y: usize) -> [u8; 4] {
    if !draw {
        return [149, 83, 17, 231];
    }
    [
        (13 * x + 7 * y + 19 * case + 89) as u8,
        (5 * x + 29 * y + 7 * case + 37) as u8,
        (17 * x + 3 * y + 11 * case + 1) as u8,
        (33 + 63 * ((x + 3 * y + case) & 3)) as u8,
    ]
}
fn region(c: Layout, pos: usize) -> &'static str {
    if pos < c.offset {
        "parent_prefix"
    } else if pos >= c.offset + c.pitch * c.height {
        "parent_suffix"
    } else if (pos - c.offset) % c.pitch >= c.width * 4 {
        "row_padding"
    } else {
        "active"
    }
}
fn expected(c: Layout, case: usize, draw: bool, pos: usize, before: &[u8]) -> u8 {
    if region(c, pos) != "active" {
        return before[pos];
    }
    let n = pos - c.offset;
    bgra(case, draw, (n % c.pitch) / 4, n / c.pitch)[n % 4]
}
fn validate_parent(raw: &[u8], case: usize, draw: bool, wrong_expected: bool) -> Result<()> {
    let c = LAYOUTS[case];
    layout_valid(c, c.pitch * c.height)?;
    need(raw.len() == STRIDE, "full parent readback length")?;
    need(
        raw[..G]
            .iter()
            .chain(raw[G + PARENT..].iter())
            .all(|&x| x == POISON),
        "readback guards",
    )?;
    let before = initial();
    for pos in 0..PARENT {
        let mut want = expected(c, case, draw, pos, &before);
        if wrong_expected && pos == c.offset {
            want ^= 1;
        }
        if raw[G + pos] != want {
            return Err(format!(
                "{} case={case} byte={pos} got={} want={want}",
                region(c, pos),
                raw[G + pos]
            ));
        }
    }
    Ok(())
}
fn capture_case(i: usize) -> usize {
    if i == 12 {
        2
    } else {
        i / 2
    }
}
fn receipt_values(i: usize, alignment: usize) -> Vec<usize> {
    let c = LAYOUTS[capture_case(i)];
    let clear = i < 12 && i % 2 == 0;
    let draw = usize::from(i < 12 && i % 2 == 1);
    vec![
        c.width,
        c.height,
        c.pitch,
        c.offset,
        c.pitch * c.height,
        PARENT,
        80,
        7,
        2,
        2,
        1,
        2,
        1,
        1,
        1,
        1,
        2,
        2,
        1,
        0,
        0,
        alignment,
        if clear { 2 } else { 1 },
        1,
        draw,
        3 * draw,
        draw,
        0,
        15,
        1,
        0,
        0,
        4,
        0,
    ]
}
fn receipt_check(s: &str) -> Result<()> {
    let lines: Vec<_> = s.lines().collect();
    need(
        s.ends_with('\n') && lines.len() == CAPTURES + 1 && lines[0] == HEADER,
        "receipt header/count",
    )?;
    let mut shared_alignment = None;
    for (i, line) in lines[1..].iter().enumerate() {
        let fields: Vec<_> = line.split('\t').collect();
        need(fields.len() == 37, "receipt field count")?;
        let op = if i == 12 {
            "skip"
        } else if i % 2 == 0 {
            "clear"
        } else {
            "draw"
        };
        need(
            fields[0] == i.to_string()
                && fields[1] == capture_case(i).to_string()
                && fields[2] == op,
            "receipt capture/order/op",
        )?;
        let alignment = fields[24]
            .parse::<usize>()
            .map_err(|_| "alignment integer")?;
        need(
            alignment.is_power_of_two() && alignment <= 256 && 256 % alignment == 0,
            "fixed alignment",
        )?;
        need(
            shared_alignment.unwrap_or(alignment) == alignment,
            "same device alignment",
        )?;
        shared_alignment = Some(alignment);
        let expected = receipt_values(i, alignment)
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>();
        need(fields[3..] == expected, "actual native getters/status")?;
    }
    Ok(())
}
fn check_snapshots(raw: &[u8]) -> Result<()> {
    need(raw.len() == CAPTURES * STRIDE, "capture byte count")?;
    for (i, chunk) in raw.chunks_exact(STRIDE).take(12).enumerate() {
        validate_parent(chunk, capture_case(i), i % 2 == 1, false)?;
    }
    let skipped = &raw[12 * STRIDE..];
    need(
        skipped == guarded(&initial()),
        "skipped draw changed parent or guards",
    )?;
    let failure = validate_parent(skipped, 2, true, false)
        .err()
        .ok_or("skipped draw incorrectly passed")?;
    need(
        failure.starts_with("active case=2 byte=256 "),
        "skipped draw rejection must be active byte",
    )
}
fn summary() -> String {
    "{\"status\":\"BOUNDED_NATIVE_STORAGE_MATCH_PENDING_MAIN_REVIEW\",\"layouts\":6,\"positive_captures\":12,\"negative_captures\":1,\"command_buffers\":1,\"active_bytes_checked\":173424,\"row_padding_bytes_checked\":46224,\"outside_view_bytes_checked\":566784,\"last_row_padding_checked\":true,\"full_parent_bytes_per_capture\":65536,\"bgra_order_checked\":true,\"skipped_draw_rejected\":true,\"readback_guard_bytes_checked\":1664,\"production_approved\":false,\"sampled_compute_qualified\":false,\"universal_metal_semantics\":false}\n".into()
}
fn generate(root: &Path) -> Result<()> {
    for c in LAYOUTS {
        layout_valid(c, c.pitch * c.height)?;
    }
    write_new(&root.join("plan.json"), plan().as_bytes())?;
    write_new(&root.join("parent-before.bin"), &initial())
}
fn acquire(root: &Path, expected_head: &str, source: &Path) -> Result<String> {
    bounds(root)?;
    binding(root, expected_head, source)?;
    need(
        read_bounded(&root.join("plan.json"))? == plan().as_bytes(),
        "exact synthetic plan",
    )?;
    need(
        read_bounded(&root.join("parent-before.bin"))? == initial(),
        "exact synthetic initial bytes",
    )?;
    need(
        read_bounded(&root.join("upload-after.bin"))? == guarded(&initial()),
        "upload after-image/guards",
    )?;
    need(
        read_bounded(&root.join("complete.txt"))? == COMPLETE.as_bytes(),
        "complete native batch",
    )?;
    let receipt =
        String::from_utf8(read_bounded(&root.join("receipts.tsv"))?).map_err(|_| "receipt UTF8")?;
    receipt_check(&receipt)?;
    check_snapshots(&read_bounded(&root.join("parents-after.bin"))?)?;
    // gpu.json is bounded diagnostic context, never a substitute for raw bytes or getters.
    need(
        !read_bounded(&root.join("gpu.json"))?.is_empty(),
        "native diagnostic missing",
    )?;
    Ok(summary())
}
fn wrong_expected_rejected(root: &Path) -> Result<()> {
    let raw = read_bounded(&root.join("parents-after.bin"))?;
    need(
        raw.len() == CAPTURES * STRIDE,
        "negative requires complete acquisition",
    )?;
    let failure = validate_parent(&raw[..STRIDE], 0, false, true)
        .err()
        .ok_or("wrong expected accepted")?;
    need(
        failure.starts_with("active case=0 byte=0 "),
        "wrong expected rejection location",
    )
}

fn write_new(path: &Path, b: &[u8]) -> Result<()> {
    need(b.len() <= 1048576, "write size")?;
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
        m.is_file() && !m.file_type().is_symlink() && m.len() <= 1048576,
        "read bounds/type",
    )?;
    fs::read(path).map_err(|e| format!("read: {}", e.kind()))
}
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
    let mut v = [
        "plan.json",
        "parent-before.bin",
        "binding.txt",
        "source-pins.tsv",
        "baseline.txt",
        "tests.txt",
        "command-exit.json",
        "receipts.tsv",
        "gpu.json",
        "complete.txt",
        "upload-after.bin",
        "parents-after.bin",
        "validation.json",
        "negative.json",
        "SHA256SUMS",
    ]
    .map(str::to_string)
    .to_vec();
    v.sort();
    v
}
fn bounds(root: &Path) -> Result<()> {
    let m = fs::symlink_metadata(root).map_err(|_| "result directory")?;
    need(
        m.is_dir() && !m.file_type().is_symlink(),
        "result directory type",
    )?;
    let names = allowed();
    let mut total = 0;
    let mut count = 0;
    for e in fs::read_dir(root).map_err(|_| "result listing")? {
        let e = e.map_err(|_| "result entry")?;
        let n = e.file_name().into_string().map_err(|_| "result name")?;
        need(
            names.binary_search(&n).is_ok(),
            "artifact outside exact allowlist",
        )?;
        let m = fs::symlink_metadata(e.path()).map_err(|_| "result metadata")?;
        need(
            m.is_file() && !m.file_type().is_symlink() && m.len() <= 1048576,
            "artifact type/size",
        )?;
        total += m.len();
        count += 1;
    }
    need(count <= 15 && total <= LIMIT, "artifact count/total")
}
fn seal_text(root: &Path) -> Result<String> {
    bounds(root)?;
    let mut files = BTreeMap::new();
    for e in fs::read_dir(root).map_err(|_| "seal listing")? {
        let e = e.map_err(|_| "seal entry")?;
        let name = e.file_name().into_string().map_err(|_| "seal name")?;
        if name != "SHA256SUMS" {
            files.insert(name, read_bounded(&e.path())?);
        }
    }
    Ok(files
        .into_iter()
        .map(|(name, b)| format!("{}  {name}\n", sha256(&b)))
        .collect())
}

fn verify(root: &Path, expected_head: &str, source: &Path) -> Result<()> {
    need(
        read_bounded(&root.join("validation.json"))?
            == acquire(root, expected_head, source)?.as_bytes(),
        "validation replay",
    )?;
    wrong_expected_rejected(root)?;
    need(
        read_bounded(&root.join("negative.json"))? == NEGATIVE.as_bytes(),
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

fn upload_guard(root: &Path, expected_head: &str, source: &Path) -> Result<()> {
    bounds(root)?;
    binding(root, expected_head, source)?;
    need(
        read_bounded(&root.join("SHA256SUMS"))? == seal_text(root)?.as_bytes(),
        "final upload seal",
    )?;
    let exit = read_bounded(&root.join("command-exit.json"))?;
    if exit == EXIT_OK.as_bytes() {
        verify(root, expected_head, source)?;
        println!("SEALED_BOUNDED_STORAGE_MATCH_PENDING_MAIN_REVIEW");
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
            "generate" if a.len() == 3 => generate(root)?,
            "bounds" if a.len() == 3 => bounds(root)?,
            "seal" if a.len() == 3 => {
                write_new(&root.join("SHA256SUMS"), seal_text(root)?.as_bytes())?;
                bounds(root)?;
            }
            "bind" if a.len() == 5 => {
                let pins = source_pins(Path::new(&a[4]), &a[3])?;
                write_new(&root.join("binding.txt"), binding_text(&a[3]).as_bytes())?;
                write_new(&root.join("source-pins.tsv"), pins.as_bytes())?;
            }
            "check" if a.len() == 5 => write_new(
                &root.join("validation.json"),
                acquire(root, &a[3], Path::new(&a[4]))?.as_bytes(),
            )?,
            "negative" if a.len() == 5 => {
                acquire(root, &a[3], Path::new(&a[4]))?;
                wrong_expected_rejected(root)?;
                write_new(&root.join("negative.json"), NEGATIVE.as_bytes())?;
                println!("WRONG_EXPECTED_REJECTED: unchanged native parent bytes");
                return Ok(2);
            }
            "verify" if a.len() == 5 => verify(root, &a[3], Path::new(&a[4]))?,
            "upload-guard" if a.len() == 5 => upload_guard(root, &a[3], Path::new(&a[4]))?,
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
    // CPU fixtures exist ONLY in unit tests; never emitted as native captures.
    fn fixture(case: usize, draw: bool) -> Vec<u8> {
        let c = LAYOUTS[case];
        let before = initial();
        guarded(
            &(0..PARENT)
                .map(|n| expected(c, case, draw, n, &before))
                .collect::<Vec<_>>(),
        )
    }
    fn receipts() -> String {
        let mut s = format!("{HEADER}\n");
        for i in 0..CAPTURES {
            let op = if i == 12 {
                "skip"
            } else if i % 2 == 0 {
                "clear"
            } else {
                "draw"
            };
            s.push_str(&format!(
                "{i}\t{}\t{op}\t{}\n",
                capture_case(i),
                receipt_values(i, 64)
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("\t")
            ));
        }
        s
    }
    #[test]
    fn fixed_small_plan_and_byte_budget() {
        assert_eq!(LAYOUTS.len(), 6);
        assert_eq!(CAPTURES * STRIDE, 853632);
        let active: usize = LAYOUTS.iter().map(|c| c.width * c.height * 4 * 2).sum();
        let padding: usize = LAYOUTS
            .iter()
            .map(|c| (c.pitch - c.width * 4) * c.height * 2)
            .sum();
        assert_eq!(
            (active, padding, 12 * PARENT - active - padding),
            (173424, 46224, 566784)
        );
        for c in LAYOUTS {
            layout_valid(c, c.pitch * c.height).unwrap();
        }
    }
    #[test]
    fn layout_rejects_alignment_range_overflow_and_short_last_row_span() {
        let c = LAYOUTS[2];
        for bad in [
            Layout { offset: 1, ..c },
            Layout { pitch: 257, ..c },
            Layout { width: 65, ..c },
            Layout { height: 257, ..c },
            Layout {
                width: usize::MAX,
                ..c
            },
            Layout {
                pitch: usize::MAX,
                ..c
            },
            Layout {
                offset: usize::MAX,
                ..c
            },
            Layout { width: 0, ..c },
        ] {
            assert!(layout_valid(bad, 768).is_err());
        }
        assert!(layout_valid(c, (c.height - 1) * c.pitch + c.width * 4).is_err());
    }
    #[test]
    fn all_synthetic_positive_images_match() {
        for c in 0..6 {
            for draw in [false, true] {
                validate_parent(&fixture(c, draw), c, draw, false).unwrap();
            }
        }
    }
    #[test]
    fn bgra_is_not_rgba_and_every_channel_is_checked() {
        assert_eq!(bgra(0, false, 0, 0), [149, 83, 17, 231]);
        assert_eq!(bgra(2, true, 4, 2), [193, 129, 97, 33]);
        for draw in [false, true] {
            for channel in 0..4 {
                let mut raw = fixture(2, draw);
                raw[G + 256 + channel] ^= 1;
                assert!(validate_parent(&raw, 2, draw, false)
                    .unwrap_err()
                    .starts_with("active"));
            }
        }
        let mut raw = fixture(2, false);
        raw.swap(G + 256, G + 258);
        assert!(validate_parent(&raw, 2, false, false).is_err());
    }
    #[test]
    fn every_row_padding_including_full_final_row_is_checked() {
        for (case, c) in LAYOUTS.iter().enumerate() {
            for y in 0..c.height {
                for pos in [
                    c.offset + y * c.pitch + c.width * 4,
                    c.offset + (y + 1) * c.pitch - 1,
                ] {
                    let mut raw = fixture(case, false);
                    raw[G + pos] ^= 1;
                    assert!(validate_parent(&raw, case, false, false)
                        .unwrap_err()
                        .starts_with("row_padding"));
                }
            }
        }
    }
    #[test]
    fn parent_prefix_suffix_and_readback_guards_are_checked() {
        let c = LAYOUTS[2];
        for (pos, label) in [
            (0, "parent_prefix"),
            (255, "parent_prefix"),
            (c.offset + c.pitch * c.height, "parent_suffix"),
            (PARENT - 1, "parent_suffix"),
        ] {
            let mut raw = fixture(2, true);
            raw[G + pos] ^= 1;
            assert!(validate_parent(&raw, 2, true, false)
                .unwrap_err()
                .starts_with(label));
        }
        for pos in [0, G - 1, G + PARENT, STRIDE - 1] {
            let mut raw = fixture(2, true);
            raw[pos] ^= 1;
            assert_eq!(
                validate_parent(&raw, 2, true, false).unwrap_err(),
                "readback guards"
            );
        }
    }
    #[test]
    fn packed_rows_wrong_pitch_and_wrong_offset_fail() {
        let c = LAYOUTS[2];
        let before = initial();
        for (pitch, offset) in [(c.width * 4, c.offset), (512, c.offset), (c.pitch, 0)] {
            let mut parent = before.clone();
            for y in 0..c.height {
                for x in 0..c.width {
                    parent[offset + y * pitch + 4 * x..offset + y * pitch + 4 * x + 4]
                        .copy_from_slice(&bgra(2, true, x, y));
                }
            }
            assert!(validate_parent(&guarded(&parent), 2, true, false).is_err());
        }
    }
    #[test]
    fn stale_poison_truncated_oversized_and_missing_rows_fail() {
        let good = fixture(2, true);
        for raw in [
            guarded(&initial()),
            vec![POISON; STRIDE],
            good[..STRIDE - 1].to_vec(),
            [good.clone(), vec![0]].concat(),
        ] {
            assert!(validate_parent(&raw, 2, true, false).is_err());
        }
        let mut raw = good;
        let before = initial();
        let c = LAYOUTS[2];
        let n = c.offset + 2 * c.pitch;
        raw[G + n..G + n + c.width * 4].copy_from_slice(&before[n..n + c.width * 4]);
        assert!(validate_parent(&raw, 2, true, false)
            .unwrap_err()
            .starts_with("active"));
    }
    #[test]
    fn single_skipped_draw_is_required_to_remain_entirely_unchanged() {
        let mut raw = (0..12)
            .flat_map(|i| fixture(capture_case(i), i % 2 == 1))
            .collect::<Vec<_>>();
        raw.extend(guarded(&initial()));
        check_snapshots(&raw).unwrap();
        raw[12 * STRIDE + G + 1000] ^= 1;
        assert!(check_snapshots(&raw).is_err());
        assert!(check_snapshots(&raw[..12 * STRIDE]).is_err());
    }
    #[test]
    fn wrong_expected_rejects_without_touching_actual_bytes() {
        let raw = fixture(0, false);
        let copy = raw.clone();
        assert!(validate_parent(&raw, 0, false, true)
            .unwrap_err()
            .starts_with("active case=0 byte=0 "));
        assert_eq!(raw, copy);
        validate_parent(&raw, 0, false, false).unwrap();
    }
    #[test]
    fn receipts_require_actual_format_usage_parent_and_completed_commands() {
        let good = receipts();
        receipt_check(&good).unwrap();
        let lines = good.lines().map(str::to_string).collect::<Vec<_>>();
        for (field, bad) in [
            (3, "2"),
            (5, "512"),
            (6, "256"),
            (7, "4"),
            (8, "65535"),
            (9, "70"),
            (10, "4"),
            (11, "0"),
            (12, "0"),
            (13, "0"),
            (14, "9"),
            (15, "2"),
            (16, "2"),
            (17, "2"),
            (18, "4"),
            (19, "0"),
            (20, "0"),
            (21, "0"),
            (22, "1"),
            (23, "1"),
            (24, "0"),
            (24, "512"),
            (25, "0"),
            (26, "0"),
            (27, "1"),
            (29, "1"),
            (30, "1"),
            (31, "7"),
            (32, "0"),
            (33, "1"),
            (34, "1"),
            (35, "3"),
            (36, "1"),
        ] {
            let mut altered = lines.clone();
            let mut row = altered[1]
                .split('\t')
                .map(str::to_string)
                .collect::<Vec<_>>();
            row[field] = bad.into();
            altered[1] = row.join("\t");
            assert!(
                receipt_check(&(altered.join("\n") + "\n")).is_err(),
                "field {field}"
            );
        }
        assert!(receipt_check(&(lines[..13].join("\n") + "\n")).is_err());
    }
    #[test]
    fn source_hash_known_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn no_partial_failure_can_be_a_success_receipt() {
        assert!(partial_failure_exit(b"{\"exit\":3,\"phase\":\"native\"}\n"));
        for bad in [
            EXIT_OK,
            "{\"exit\":0,\"phase\":\"native\"}\n",
            "{\"exit\":256,\"phase\":\"native\"}\n",
            "{\"exit\":3,\"phase\":\"unknown\"}\n",
        ] {
            assert!(!partial_failure_exit(bad.as_bytes()));
        }
    }
}
