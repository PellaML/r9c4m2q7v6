// Owned synthetic BGRA8 usage-7 buffer-backed SAMPLING probe. No dependencies or CPU fallback.
// Rust owns inputs, coordinates, expected values and every comparison. Source binding,
// SHA-256 and bounded artifact guards reuse the existing public harness unchanged.
#![forbid(unsafe_code)]
use std::{collections::BTreeMap, env, fs, io::Write, path::Path, process::Command};
type Result<T> = std::result::Result<T, String>;
const SOURCE: [&str; 5] = [
    ".github/workflows/bgra-sampled.yml",
    "scripts/bgra-sampled/cases.rs",
    "scripts/bgra-sampled/probe.metal",
    "scripts/bgra-sampled/probe.swift",
    "scripts/bgra-sampled/run.sh",
];
const PARENT: usize = 65536;
const G: usize = 64;
const STRIDE: usize = PARENT + G * 2;
const CAPTURES: usize = 9;
const K: usize = 64;
const POISON: u8 = 0xcd;
const LIMIT: u64 = 2 * 1024 * 1024;
// Byte units (value * 255). Read/nearest must identify the exact stored byte;
// linear is a bounded filter tolerance, not an exactness claim.
const READ_TOL: f64 = 0.25;
const LINEAR_TOL: f64 = 2.0;
#[cfg(test)]
const DISCRIMINATE: f64 = 2.0 * LINEAR_TOL;
const COMPLETE: &str = "schema=1\ncommands=3\ncaptures=9\nsample_passes=14\ncompute_dispatches=42\ndraws=4\nclears=4\nempty_load_store_passes=1\nblit_copies=18\n";
const EXIT_OK: &str = "{\"exit\":0,\"phase\":\"complete\"}\n";
const NEGATIVE: &str = "{\"wrong_expected_rejected\":true,\"native_bytes_unchanged\":true,\"capture\":0,\"pixel\":0,\"exit\":2}\n";
const HEADER: &str = "capture\tcase\top\twidth\theight\tpitch\toffset\tview_bytes\tparent_bytes\tformat\tusage\tstorage\tparent_storage\tparent_identity\ttype\tdepth\tmips\tslices\tsamples\thazard\tparent_hazard\tidentity_swizzle\ttexture_parent\tframebuffer_only\talignment\tpasses\tdraws\tclears\tnearest_points\tlinear_points\tstatus\terror";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    width: usize,
    height: usize,
    pitch: usize,
    offset: usize,
}
const LAYOUTS: [Layout; 4] = [
    Layout { width: 1, height: 1, pitch: 256, offset: 0 },
    Layout { width: 5, height: 3, pitch: 256, offset: 256 },
    Layout { width: 5, height: 3, pitch: 512, offset: 256 },
    Layout { width: 104, height: 104, pitch: 512, offset: 0 },
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Content {
    Initial,
    Draw,
    Clear,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    SampledFirst,
    RenderFirst,
    Skip,
}
fn need(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn capture(i: usize) -> (usize, Op) {
    if i == 8 {
        (1, Op::Skip)
    } else if i % 2 == 0 {
        (i / 2, Op::SampledFirst)
    } else {
        (i / 2, Op::RenderFirst)
    }
}
fn op_name(op: Op) -> &'static str {
    match op {
        Op::SampledFirst => "sampled_first_draw",
        Op::RenderFirst => "render_first_clear",
        Op::Skip => "sampled_first_skip",
    }
}
fn passes(op: Op) -> &'static [Content] {
    match op {
        Op::SampledFirst => &[Content::Initial, Content::Draw],
        Op::RenderFirst => &[Content::Clear],
        Op::Skip => &[Content::Initial, Content::Initial],
    }
}
fn parent_content(op: Op) -> Content {
    match op {
        Op::SampledFirst => Content::Draw,
        Op::RenderFirst => Content::Clear,
        Op::Skip => Content::Initial,
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
    format!("{{\"schema\":1,\"parent\":65536,\"guard_bytes\":64,\"captures\":9,\"points\":64,\"clear_rgba\":[17,83,149,231],\"layouts\":[{layouts}]}}\n")
}
fn initial_byte(i: usize) -> u8 {
    ((i * 73) ^ ((i >> 8) * 19) ^ 0xa5) as u8
}
fn initial() -> Vec<u8> {
    (0..PARENT).map(initial_byte).collect()
}
fn guarded(data: &[u8]) -> Vec<u8> {
    [vec![POISON; G], data.to_vec(), vec![POISON; G]].concat()
}
// Physical BGRA bytes of the owned fragment pattern (same formula as probe.metal).
fn draw_bgra(case: usize, x: usize, y: usize) -> [u8; 4] {
    [
        (13 * x + 7 * y + 19 * case + 89) as u8,
        (5 * x + 29 * y + 7 * case + 37) as u8,
        (17 * x + 3 * y + 11 * case + 1) as u8,
        (33 + 63 * ((x + 3 * y + case) & 3)) as u8,
    ]
}
fn texel(case: usize, content: Content, x: usize, y: usize) -> [u8; 4] {
    let c = LAYOUTS[case];
    match content {
        Content::Initial => {
            let p = c.offset + y * c.pitch + 4 * x;
            [initial_byte(p), initial_byte(p + 1), initial_byte(p + 2), initial_byte(p + 3)]
        }
        Content::Draw => draw_bgra(case, x, y),
        Content::Clear => [149, 83, 17, 231],
    }
}
// Shader-visible RGBA in byte units for an identity-swizzle BGRA8Unorm texel.
fn rgba(case: usize, content: Content, x: usize, y: usize) -> [f64; 4] {
    let t = texel(case, content, x, y);
    [f64::from(t[2]), f64::from(t[1]), f64::from(t[0]), f64::from(t[3])]
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
fn expected(case: usize, content: Content, pos: usize) -> u8 {
    let c = LAYOUTS[case];
    if region(c, pos) != "active" {
        return initial_byte(pos);
    }
    let n = pos - c.offset;
    texel(case, content, (n % c.pitch) / 4, n / c.pitch)[n % 4]
}
fn validate_parent(raw: &[u8], case: usize, content: Content) -> Result<()> {
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
    for pos in 0..PARENT {
        let want = expected(case, content, pos);
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
// Normalized coordinates. Nearest points are texel centres or at least a quarter
// texel outside the image (clamp_to_edge). Linear points are half/quarter texel
// positions plus exact right/bottom/corner edges where a padding leak would show.
fn points(case: usize) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let c = LAYOUTS[case];
    let (w, h) = (c.width as f64, c.height as f64);
    let mut n: Vec<[f64; 2]> = vec![
        [-0.25, 0.5 / h],
        [1.25, (h - 0.5) / h],
        [(w - 0.5) / w, 1.25],
        [1.25, 1.25],
        [-0.25, 1.25],
        [(w - 0.5) / w, (h - 0.5) / h],
        [0.5 / w, (h - 0.5) / h],
        [(w - 0.5) / w, 0.5 / h],
    ];
    for j in 8..K {
        let x = ((j * 37 + 11) % c.width) as f64;
        let y = ((j * 53 + 5) % c.height) as f64;
        n.push([(x + 0.5) / w, (y + 0.5) / h]);
    }
    let mut l: Vec<[f64; 2]> = vec![
        [1.0, 0.5 / h],
        [1.0, (h - 0.5) / h],
        [(w - 0.5) / w, 1.0],
        [1.0, 1.0],
        [0.0, 0.0],
        [1.25, 1.25],
        [-0.25, 0.5 / h],
        [0.0, 1.0],
    ];
    for j in 8..K {
        let x = ((j * 29 + 7) % c.width) as f64;
        let y = ((j * 31 + 3) % c.height) as f64;
        l.push(match j % 4 {
            0 => [(x + 1.0) / w, (y + 0.5) / h],
            1 => [(x + 0.5) / w, (y + 1.0) / h],
            2 => [(x + 1.0) / w, (y + 1.0) / h],
            _ => [(x + 0.75) / w, (y + 0.25) / h],
        });
    }
    let single = |v: Vec<[f64; 2]>| -> Vec<[f32; 2]> {
        v.into_iter().map(|p| [p[0] as f32, p[1] as f32]).collect()
    };
    (single(n), single(l))
}
fn coords() -> Vec<u8> {
    let mut b = Vec::new();
    for case in 0..LAYOUTS.len() {
        let (n, l) = points(case);
        for p in n.iter().chain(l.iter()) {
            b.extend(p[0].to_le_bytes());
            b.extend(p[1].to_le_bytes());
        }
    }
    b
}
fn clamp_index(t: f64, n: usize) -> usize {
    if t < 0.0 {
        0
    } else if t >= n as f64 {
        n - 1
    } else {
        t as usize
    }
}
fn nearest(case: usize, content: Content, p: [f32; 2]) -> [f64; 4] {
    let c = LAYOUTS[case];
    let x = clamp_index((f64::from(p[0]) * c.width as f64).floor(), c.width);
    let y = clamp_index((f64::from(p[1]) * c.height as f64).floor(), c.height);
    rgba(case, content, x, y)
}
// Bilinear clamp_to_edge expectation. `leak` is a test-only alternative that
// reads row-padding bytes instead of clamping at the right edge.
fn linear(case: usize, content: Content, p: [f32; 2], leak: bool) -> [f64; 4] {
    let c = LAYOUTS[case];
    let tx = f64::from(p[0]) * c.width as f64 - 0.5;
    let ty = f64::from(p[1]) * c.height as f64 - 0.5;
    let (x0, y0) = (tx.floor(), ty.floor());
    let (fx, fy) = (tx - x0, ty - y0);
    let at = |x: f64, y: f64| -> [f64; 4] {
        let yy = clamp_index(y, c.height);
        if leak && x >= c.width as f64 && (x as usize) * 4 < c.pitch {
            let q = c.offset + yy * c.pitch + 4 * (x as usize);
            return [initial_byte(q + 2), initial_byte(q + 1), initial_byte(q), initial_byte(q + 3)]
                .map(f64::from);
        }
        rgba(case, content, clamp_index(x, c.width), yy)
    };
    let (a, b, d, e) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
    let mut out = [0.0; 4];
    for (k, o) in out.iter_mut().enumerate() {
        *o = a[k] * (1.0 - fx) * (1.0 - fy)
            + b[k] * fx * (1.0 - fy)
            + d[k] * (1.0 - fx) * fy
            + e[k] * fx * fy;
    }
    out
}
// One pass = read() of every texel, then K nearest samples, then K linear samples, float4 LE.
fn pass_bytes(case: usize) -> usize {
    let c = LAYOUTS[case];
    (c.width * c.height + 2 * K) * 16
}
fn samples_total() -> usize {
    (0..CAPTURES)
        .map(|i| {
            let (case, op) = capture(i);
            pass_bytes(case) * passes(op).len()
        })
        .sum()
}
fn f32_at(raw: &[u8], i: usize) -> f64 {
    f64::from(f32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]]))
}
fn compare(kind: &str, case: usize, index: usize, raw: &[u8], want: [f64; 4], tol: f64) -> Result<()> {
    for (ch, w) in want.iter().enumerate() {
        let g = f32_at(raw, 4 * index + ch) * 255.0;
        if !g.is_finite() || (g - w).abs() > tol {
            return Err(format!("{kind} case={case} index={index} channel={ch} got={g:.4} want={w:.4}"));
        }
    }
    Ok(())
}
fn validate_pass(raw: &[u8], case: usize, content: Content, wrong_expected: bool) -> Result<()> {
    let c = LAYOUTS[case];
    let texels = c.width * c.height;
    need(raw.len() == pass_bytes(case), "sample pass length")?;
    for y in 0..c.height {
        for x in 0..c.width {
            let i = y * c.width + x;
            let mut want = rgba(case, content, x, y);
            if wrong_expected && i == 0 {
                want[0] = (want[0] + 1.0) % 256.0;
            }
            compare("read", case, i, raw, want, READ_TOL)?;
        }
    }
    let (near, lin) = points(case);
    for (j, p) in near.iter().enumerate() {
        compare("nearest", case, texels + j, raw, nearest(case, content, *p), READ_TOL)?;
    }
    for (j, p) in lin.iter().enumerate() {
        compare("linear", case, texels + K + j, raw, linear(case, content, *p, false), LINEAR_TOL)?;
    }
    Ok(())
}
fn check_samples(raw: &[u8]) -> Result<()> {
    need(raw.len() == samples_total(), "sample byte count")?;
    let mut at = 0;
    for i in 0..CAPTURES {
        let (case, op) = capture(i);
        for (k, content) in passes(op).iter().enumerate() {
            let pass = &raw[at..at + pass_bytes(case)];
            validate_pass(pass, case, *content, false).map_err(|e| format!("capture={i} pass={k} {e}"))?;
            if op == Op::Skip && k == 1 {
                let failure = validate_pass(pass, case, Content::Draw, false)
                    .err()
                    .ok_or("skipped draw sampled as drawn")?;
                need(
                    failure.starts_with("read case=1 index=0 "),
                    "skipped draw rejection must be first texel",
                )?;
            }
            at += pass_bytes(case);
        }
    }
    Ok(())
}
fn receipt_values(i: usize, alignment: usize) -> Vec<usize> {
    let (case, op) = capture(i);
    let c = LAYOUTS[case];
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
        passes(op).len(),
        usize::from(op == Op::SampledFirst),
        usize::from(op == Op::RenderFirst),
        K,
        K,
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
        need(fields.len() == 32, "receipt field count")?;
        let (case, op) = capture(i);
        need(
            fields[0] == i.to_string() && fields[1] == case.to_string() && fields[2] == op_name(op),
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
    for (i, chunk) in raw.chunks_exact(STRIDE).enumerate() {
        let (case, op) = capture(i);
        validate_parent(chunk, case, parent_content(op)).map_err(|e| format!("capture={i} {e}"))?;
    }
    let skipped = &raw[8 * STRIDE..];
    need(
        skipped == guarded(&initial()).as_slice(),
        "skipped draw changed parent or guards",
    )?;
    let failure = validate_parent(skipped, 1, Content::Draw)
        .err()
        .ok_or("skipped draw incorrectly passed")?;
    need(
        failure.starts_with("active case=1 byte=256 "),
        "skipped draw rejection must be active byte",
    )
}
fn summary() -> String {
    "{\"status\":\"BOUNDED_NATIVE_SAMPLED_MATCH_PENDING_MAIN_REVIEW\",\"layouts\":4,\"captures\":9,\"command_buffers\":3,\"sample_passes\":14,\"read_texels_checked\":32571,\"nearest_points_checked\":896,\"linear_points_checked\":896,\"read_nearest_tolerance\":\"0.25/255\",\"linear_tolerance\":\"2/255\",\"active_bytes_checked\":86836,\"row_padding_bytes_checked\":25548,\"outside_view_bytes_checked\":477440,\"bgra_order_checked\":true,\"edge_clamp_without_padding_checked\":true,\"sampled_first_then_render_checked\":true,\"render_first_then_sample_checked\":true,\"skipped_draw_rejected\":true,\"readback_guard_bytes_checked\":1152,\"compute_kernel_sampling_only\":true,\"fragment_sampling_qualified\":false,\"production_approved\":false,\"universal_metal_semantics\":false}\n".into()
}
fn generate(root: &Path) -> Result<()> {
    for c in LAYOUTS {
        layout_valid(c, c.pitch * c.height)?;
    }
    write_new(&root.join("plan.json"), plan().as_bytes())?;
    write_new(&root.join("parent-before.bin"), &initial())?;
    write_new(&root.join("coords.bin"), &coords())
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
        read_bounded(&root.join("coords.bin"))? == coords(),
        "exact synthetic coordinates",
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
    check_samples(&read_bounded(&root.join("samples.bin"))?)?;
    // gpu.json is bounded diagnostic context, never a substitute for raw bytes or getters.
    need(
        !read_bounded(&root.join("gpu.json"))?.is_empty(),
        "native diagnostic missing",
    )?;
    Ok(summary())
}
fn wrong_expected_rejected(root: &Path) -> Result<()> {
    let raw = read_bounded(&root.join("samples.bin"))?;
    need(
        raw.len() == samples_total(),
        "negative requires complete acquisition",
    )?;
    let first = &raw[..pass_bytes(0)];
    validate_pass(first, 0, Content::Initial, false)?;
    let failure = validate_pass(first, 0, Content::Initial, true)
        .err()
        .ok_or("wrong expected accepted")?;
    need(
        failure.starts_with("read case=0 index=0 channel=0 "),
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
        "coords.bin",
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
        "samples.bin",
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
    need(count <= 17 && total <= LIMIT, "artifact count/total")
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
        println!("SEALED_BOUNDED_SAMPLED_MATCH_PENDING_MAIN_REVIEW");
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
    fn parent_fixture(case: usize, content: Content) -> Vec<u8> {
        guarded(&(0..PARENT).map(|p| expected(case, content, p)).collect::<Vec<_>>())
    }
    fn pass_values(case: usize, content: Content) -> Vec<[f64; 4]> {
        let c = LAYOUTS[case];
        let mut v = Vec::new();
        for y in 0..c.height {
            for x in 0..c.width {
                v.push(rgba(case, content, x, y));
            }
        }
        let (n, l) = points(case);
        v.extend(n.iter().map(|p| nearest(case, content, *p)));
        v.extend(l.iter().map(|p| linear(case, content, *p, false)));
        v
    }
    fn encode(v: &[[f64; 4]]) -> Vec<u8> {
        v.iter()
            .flat_map(|q| q.iter().flat_map(|x| ((x / 255.0) as f32).to_le_bytes()))
            .collect()
    }
    fn pass_fixture(case: usize, content: Content) -> Vec<u8> {
        encode(&pass_values(case, content))
    }
    fn set_value(raw: &mut [u8], index: usize, ch: usize, byte_units: f64) {
        let at = 4 * (4 * index + ch);
        raw[at..at + 4].copy_from_slice(&((byte_units / 255.0) as f32).to_le_bytes());
    }
    fn samples_fixture() -> Vec<u8> {
        let mut raw = Vec::new();
        for i in 0..CAPTURES {
            let (case, op) = capture(i);
            for content in passes(op) {
                raw.extend(pass_fixture(case, *content));
            }
        }
        raw
    }
    fn parents_fixture() -> Vec<u8> {
        (0..CAPTURES)
            .flat_map(|i| {
                let (case, op) = capture(i);
                parent_fixture(case, parent_content(op))
            })
            .collect()
    }
    fn receipts() -> String {
        let mut s = format!("{HEADER}\n");
        for i in 0..CAPTURES {
            let (case, op) = capture(i);
            s.push_str(&format!(
                "{i}\t{case}\t{}\t{}\n",
                op_name(op),
                receipt_values(i, 64)
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("\t")
            ));
        }
        s
    }
    fn max_diff(a: [f64; 4], b: [f64; 4]) -> f64 {
        (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f64::max)
    }
    #[test]
    fn fixed_small_plan_and_byte_budget() {
        let (mut n_passes, mut texels, mut active, mut padding) = (0, 0, 0, 0);
        for i in 0..CAPTURES {
            let (case, op) = capture(i);
            let c = LAYOUTS[case];
            layout_valid(c, c.pitch * c.height).unwrap();
            n_passes += passes(op).len();
            texels += c.width * c.height * passes(op).len();
            active += c.width * c.height * 4;
            padding += (c.pitch - c.width * 4) * c.height;
        }
        assert_eq!(
            (n_passes, texels, active, padding, CAPTURES * PARENT - active - padding),
            (14, 32571, 86836, 25548, 477440)
        );
        assert_eq!(samples_total(), 549808);
        assert_eq!(CAPTURES * STRIDE, 590976);
        assert_eq!(coords().len(), LAYOUTS.len() * 2 * K * 8);
        assert_eq!(&coords()[..4], &(-0.25f32).to_le_bytes());
        // The production-like class: 104x104, pitch 512, full span 53248 bytes.
        assert_eq!(LAYOUTS[3].pitch * LAYOUTS[3].height, 53248);
        assert!(samples_total() as u64 + (CAPTURES * STRIDE) as u64 + 3 * 65664 < LIMIT);
    }
    #[test]
    fn all_synthetic_positive_fixtures_match() {
        check_samples(&samples_fixture()).unwrap();
        check_snapshots(&parents_fixture()).unwrap();
        receipt_check(&receipts()).unwrap();
    }
    #[test]
    fn nearest_points_are_unambiguous_centres_or_clamped() {
        for (case, c) in LAYOUTS.iter().enumerate() {
            for p in points(case).0 {
                for (t, size) in [
                    (f64::from(p[0]) * c.width as f64, c.width as f64),
                    (f64::from(p[1]) * c.height as f64, c.height as f64),
                ] {
                    let f = t - t.floor();
                    assert!(
                        t <= -0.25 || t >= size + 0.25 || (0.25..=0.75).contains(&f),
                        "case {case} {t}"
                    );
                }
            }
        }
    }
    #[test]
    fn linear_expectations_are_distinguishable_from_nearest() {
        for case in 1..LAYOUTS.len() {
            for content in [Content::Initial, Content::Draw] {
                let count = points(case)
                    .1
                    .iter()
                    .filter(|p| {
                        max_diff(linear(case, content, **p, false), nearest(case, content, **p))
                            > DISCRIMINATE
                    })
                    .count();
                assert!(count >= 40, "case {case} {content:?} {count}");
            }
        }
        // A nearest-filtered native result must therefore fail the linear checks.
        let mut v = pass_values(1, Content::Initial);
        let (_, l) = points(1);
        for (j, p) in l.iter().enumerate() {
            v[15 + K + j] = nearest(1, Content::Initial, *p);
        }
        assert!(validate_pass(&encode(&v), 1, Content::Initial, false)
            .unwrap_err()
            .starts_with("linear"));
    }
    #[test]
    fn edge_clamp_must_not_blend_row_padding() {
        for case in 0..LAYOUTS.len() {
            for content in [Content::Initial, Content::Draw, Content::Clear] {
                let (_, l) = points(case);
                for p in &l[..2] {
                    assert!(
                        max_diff(linear(case, content, *p, false), linear(case, content, *p, true))
                            > DISCRIMINATE,
                        "case {case} {content:?}"
                    );
                }
                let mut v = pass_values(case, content);
                let texels = LAYOUTS[case].width * LAYOUTS[case].height;
                v[texels + K] = linear(case, content, l[0], true);
                assert!(validate_pass(&encode(&v), case, content, false)
                    .unwrap_err()
                    .starts_with(&format!("linear case={case} index={} ", texels + K)));
            }
        }
    }
    #[test]
    fn bgra_is_not_rgba_in_parent_or_shader_values() {
        assert_eq!(rgba(1, Content::Draw, 0, 0), [12.0, 44.0, 108.0, 96.0]);
        assert_eq!(texel(1, Content::Clear, 4, 2), [149, 83, 17, 231]);
        for content in [Content::Initial, Content::Draw, Content::Clear] {
            let mut v = pass_values(1, content);
            v[0] = [v[0][2], v[0][1], v[0][0], v[0][3]];
            assert!(validate_pass(&encode(&v), 1, content, false)
                .unwrap_err()
                .starts_with("read case=1 index=0 channel=0 "));
            for ch in 0..4 {
                let mut raw = pass_fixture(1, content);
                let want = rgba(1, content, 0, 0)[ch];
                set_value(&mut raw, 0, ch, (want + 1.0) % 256.0);
                assert!(validate_pass(&raw, 1, content, false).is_err());
            }
        }
        let mut raw = parent_fixture(1, Content::Draw);
        raw.swap(G + 256, G + 258);
        assert!(validate_parent(&raw, 1, Content::Draw).is_err());
    }
    #[test]
    fn read_and_linear_tolerances_are_bounded() {
        let base = pass_fixture(1, Content::Draw);
        let want = rgba(1, Content::Draw, 0, 0)[0];
        let lin = linear(1, Content::Draw, points(1).1[8], false)[0];
        for (index, value, ok) in [
            (0, want + 0.2, true),
            (0, want + 0.3, false),
            (0, f64::NAN, false),
            (15 + K + 8, lin + 1.9, true),
            (15 + K + 8, lin + 2.1, false),
        ] {
            let mut raw = base.clone();
            set_value(&mut raw, index, 0, value);
            assert_eq!(validate_pass(&raw, 1, Content::Draw, false).is_ok(), ok, "{index} {value}");
        }
        assert!(validate_pass(&base[..base.len() - 1], 1, Content::Draw, false).is_err());
    }
    #[test]
    fn every_row_padding_prefix_suffix_and_guard_is_checked() {
        for (case, c) in LAYOUTS.iter().enumerate() {
            for y in 0..c.height {
                for pos in [c.offset + y * c.pitch + c.width * 4, c.offset + (y + 1) * c.pitch - 1] {
                    let mut raw = parent_fixture(case, Content::Draw);
                    raw[G + pos] ^= 1;
                    assert!(validate_parent(&raw, case, Content::Draw)
                        .unwrap_err()
                        .starts_with("row_padding"));
                }
            }
        }
        let c = LAYOUTS[2];
        for (pos, label) in [
            (0, "parent_prefix"),
            (255, "parent_prefix"),
            (c.offset + c.pitch * c.height, "parent_suffix"),
            (PARENT - 1, "parent_suffix"),
        ] {
            let mut raw = parent_fixture(2, Content::Clear);
            raw[G + pos] ^= 1;
            assert!(validate_parent(&raw, 2, Content::Clear)
                .unwrap_err()
                .starts_with(label));
        }
        for pos in [0, G - 1, G + PARENT, STRIDE - 1] {
            let mut raw = parent_fixture(2, Content::Clear);
            raw[pos] ^= 1;
            assert_eq!(validate_parent(&raw, 2, Content::Clear).unwrap_err(), "readback guards");
        }
    }
    #[test]
    fn wrong_pitch_offset_and_stale_parents_fail() {
        let c = LAYOUTS[2];
        for (pitch, offset) in [(c.width * 4, c.offset), (256, c.offset), (c.pitch, 0)] {
            let mut parent = initial();
            for y in 0..c.height {
                for x in 0..c.width {
                    let at = offset + y * pitch + 4 * x;
                    parent[at..at + 4].copy_from_slice(&draw_bgra(2, x, y));
                }
            }
            assert!(validate_parent(&guarded(&parent), 2, Content::Draw).is_err());
        }
        for raw in [guarded(&initial()), vec![POISON; STRIDE], parent_fixture(2, Content::Draw)[..STRIDE - 1].to_vec()] {
            assert!(validate_parent(&raw, 2, Content::Draw).is_err());
        }
    }
    #[test]
    fn skipped_draw_and_wrong_capture_order_are_rejected() {
        let good = samples_fixture();
        check_samples(&good).unwrap();
        // Replace the skip control's second pass with drawn values: must fail.
        let tail = pass_bytes(1);
        let mut drawn = good.clone();
        let n = drawn.len();
        drawn[n - tail..].copy_from_slice(&pass_fixture(1, Content::Draw));
        assert!(check_samples(&drawn).is_err());
        // Sampled-first pre-pass must see initial bytes, not the later draw.
        let mut early = good.clone();
        early[..pass_bytes(0)].copy_from_slice(&pass_fixture(0, Content::Draw));
        assert!(check_samples(&early).is_err());
        assert!(check_samples(&good[..n - 16]).is_err());
        let mut parents = parents_fixture();
        parents[8 * STRIDE + G + 1000] ^= 1;
        assert!(check_snapshots(&parents).is_err());
        assert!(check_snapshots(&parents_fixture()[..8 * STRIDE]).is_err());
    }
    #[test]
    fn wrong_expected_rejects_without_touching_actual_bytes() {
        let raw = pass_fixture(0, Content::Initial);
        let copy = raw.clone();
        assert!(validate_pass(&raw, 0, Content::Initial, true)
            .unwrap_err()
            .starts_with("read case=0 index=0 channel=0 "));
        assert_eq!(raw, copy);
        validate_pass(&raw, 0, Content::Initial, false).unwrap();
    }
    #[test]
    fn receipts_require_actual_format_usage_parent_counts_and_status() {
        let good = receipts();
        receipt_check(&good).unwrap();
        let lines = good.lines().map(str::to_string).collect::<Vec<_>>();
        for row in [1, 9] {
            for field in 3..32 {
                let mut altered = lines.clone();
                let mut cells = altered[row].split('\t').map(str::to_string).collect::<Vec<_>>();
                cells[field] = (cells[field].parse::<usize>().unwrap() + 1).to_string();
                altered[row] = cells.join("\t");
                assert!(receipt_check(&(altered.join("\n") + "\n")).is_err(), "row {row} field {field}");
            }
        }
        let mut swapped = lines.clone();
        swapped[1] = swapped[1].replacen("sampled_first_draw", "render_first_clear", 1);
        assert!(receipt_check(&(swapped.join("\n") + "\n")).is_err());
        assert!(receipt_check(&(lines[..CAPTURES].join("\n") + "\n")).is_err());
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
