use std::{
    env, fs, io,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const VERSION: &str = "generic-rg16-3";
const SOURCE: &str = include_str!("probe.metal");
const PIXELS: usize = 64;
const SOURCES: [(&str, &[u8]); 5] = [
    ("scripts/rg16/host.rs", include_bytes!("host.rs")),
    ("scripts/rg16/bridge.m", include_bytes!("bridge.m")),
    ("scripts/rg16/probe.metal", include_bytes!("probe.metal")),
    ("scripts/rg16/run.sh", include_bytes!("run.sh")),
    (
        ".github/workflows/rg16.yml",
        include_bytes!("../../.github/workflows/rg16.yml"),
    ),
];
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn obj(fields: &[(&str, String)]) -> String {
    format!(
        "{{{}}}",
        fields
            .iter()
            .map(|(k, v)| format!("{}:{v}", quote(k)))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn array(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}
fn write_json(path: impl AsRef<Path>, text: &str) -> io::Result<()> {
    fs::write(path, format!("{text}\n"))
}
fn hex(x: u32) -> String {
    quote(&format!("0x{x:08x}"))
}
fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|x| x.to_le_bytes()).collect()
}

// Dependency-free SHA-256; receipt integrity, not an implementation of GPU math.
fn sha256(data: &[u8]) -> String {
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
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, b) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(b.try_into().unwrap());
        }
        for i in 16..64 {
            let a = w[i - 15];
            let b = w[i - 2];
            w[i] = w[i - 16]
                .wrapping_add(a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3))
                .wrapping_add(w[i - 7])
                .wrapping_add(b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10));
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut z] = h;
        for i in 0..64 {
            let t = z
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let u = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            z = g;
            g = f;
            f = e;
            e = d.wrapping_add(t);
            d = c;
            c = b;
            b = a;
            a = t.wrapping_add(u);
        }
        for (v, x) in h.iter_mut().zip([a, b, c, d, e, f, g, z]) {
            *v = v.wrapping_add(x);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}
fn digest(name: &str, data: &[u8]) -> String {
    obj(&[
        ("file", quote(name)),
        ("byte_length", data.len().to_string()),
        ("sha256", quote(&sha256(data))),
    ])
}
fn source_receipts() -> String {
    array(SOURCES.iter().map(|(n, b)| digest(n, b)))
}

// Integer-only binary32 -> binary16 reference, ties to even. The table tests
// below use literal independent anchors. NaN payload/sign is not asserted.
fn half(b: u32) -> u16 {
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 255) as i32;
    let frac = b & 0x7fffff;
    if exp == 255 {
        return sign | if frac == 0 { 0x7c00 } else { 0x7e00 };
    }
    let e = exp - 127;
    if e > 15 {
        return sign | 0x7c00;
    }
    if e < -25 {
        return sign;
    }
    let sig = frac | 0x800000;
    let shift = if e < -14 { (-e - 1) as u32 } else { 13 };
    let base = sig >> shift;
    let rem = sig & ((1u32 << shift) - 1);
    let midpoint = 1u32 << (shift - 1);
    let rounded = base + u32::from(rem > midpoint || (rem == midpoint && base & 1 != 0));
    if e < -14 {
        sign | rounded as u16
    } else {
        sign | ((((e + 14) as u32) << 10) + rounded) as u16
    }
}
fn widen(h: u16) -> u32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exp = ((h >> 10) & 31) as i32;
    let frac = (h & 1023) as u32;
    if exp == 31 {
        return sign | 0x7f800000 | (frac << 13);
    }
    if exp == 0 {
        if frac == 0 {
            return sign;
        }
        let k = 31 - frac.leading_zeros();
        return sign | ((k + 103) << 23) | ((frac - (1 << k)) << (23 - k));
    }
    sign | (((exp + 112) as u32) << 23) | (frac << 13)
}
fn pack(r: f32, g: f32) -> u32 {
    half(r.to_bits()) as u32 | ((half(g.to_bits()) as u32) << 16)
}
fn nan16(h: u16) -> bool {
    h & 0x7c00 == 0x7c00 && h & 1023 != 0
}
fn component_matches(expected: u16, actual: u16) -> bool {
    if nan16(expected) {
        nan16(actual) || actual & 0x7fff == 0
    } else {
        expected == actual
    }
}
// Every value is independent of any application. Adjacent binary32 words test
// below/at/above half ties; endpoint cases test signed zero, denormals and range.
const EDGES: [u32; 36] = [
    0x00000000, 0x80000000, 0x3f800000, 0xbf800000, 0x3e800000, 0x3fc00000, 0x3f800fff, 0x3f801000,
    0x3f801001, 0x3f802fff, 0x3f803000, 0x3f803001, 0xbf800fff, 0xbf801000, 0xbf801001, 0xbf802fff,
    0xbf803000, 0xbf803001, 0x33000000, 0x33000001, 0x33800000, 0x33c00000, 0xb3000000, 0xb3800000,
    0x387fc000, 0x387fe000, 0x38800000, 0xb8800000, 0x477fe000, 0x477fefff, 0x477ff000, 0xc77ff000,
    0x7f800000, 0xff800000, 0x7fc00001, 0xffc12345,
];
const CLEAR_BITS: [[u32; 2]; 12] = [
    [0x3e800000, 0xbfc00000],
    [0x00000000, 0x80000000],
    [0x3f801000, 0x3f803000],
    [0xbf801000, 0xbf803000],
    [0x3f800fff, 0x3f801001],
    [0x33000000, 0x33000001],
    [0x33800000, 0x33c00000],
    [0xb3000000, 0xb3800000],
    [0x387fc000, 0x387fe000],
    [0x38800000, 0xb8800000],
    [0x477fe000, 0x477fefff],
    [0x477ff000, 0xc77ff000],
];
fn count(s: u32) -> usize {
    match s {
        0 => CLEAR_BITS.len(),
        1 | 4 | 6 => 1,
        2 => 7,
        3 => 4,
        5 => 2,
        _ => 0,
    }
}
fn name(s: u32) -> &'static str {
    match s {
        0 => "clear",
        1 => "render",
        2 => "masks",
        3 => "blend",
        4 => "sample",
        5 => "mixed",
        6 => "lod-average",
        _ => "invalid",
    }
}
const SAMPLE_X: [f32; 16] = [
    -0.75, 0.25, 0.75, 1.25, 2.25, 3.75, 5.25, 7.75, 9.25, 11.75, 13.25, 14.75, 15.25, 31.25, 0.0,
    8.5,
];
const SAMPLE_Y: [f32; 4] = [-0.75, 0.25, 1.75, 3.25];
fn colors(s: u32) -> (Vec<f32>, Vec<f32>) {
    let mut src = Vec::new();
    let mut dst = Vec::new();
    for i in 0..PIXELS {
        let x = (i % 16) as f32;
        let y = (i / 16) as f32;
        let a = [0.0, 0.25, 0.5, 0.75, 1.0][i % 5];
        let c = if s == 1 {
            [
                f32::from_bits(EDGES[i % EDGES.len()]),
                f32::from_bits(EDGES[(7 * i + 5) % EDGES.len()]),
                -5.0,
                a,
            ]
        } else {
            [(x + 1.0) / 16.0, 0.25 + y / 8.0, -5.0, a]
        };
        src.extend_from_slice(&c);
        if s == 6 {
            dst.extend_from_slice(&[
                (SAMPLE_X[i % 16] + 0.5) / 16.0,
                (SAMPLE_Y[i / 16] + 0.5) / 4.0,
                [1.0 / 1024.0, 0.25, 4.0, 16.0][i % 4],
                [1.0 / 256.0, 0.5, 8.0, 32.0][(i / 4) % 4],
            ]);
        } else {
            dst.extend_from_slice(&[
                0.125 + y / 8.0,
                0.75 - x / 32.0,
                9.0,
                [0.0, 0.25, 0.5, 0.75][i % 4],
            ]);
        }
    }
    (src, dst)
}
fn clears() -> Vec<f64> {
    CLEAR_BITS
        .iter()
        .flatten()
        .map(|&b| f32::from_bits(b) as f64)
        .collect()
}
fn expected(s: u32) -> (Vec<u32>, Vec<u32>) {
    let (src, dst) = colors(s);
    let mut words = Vec::new();
    let mut aux = Vec::new();
    for k in 0..count(s) {
        for i in 0..PIXELS {
            let c = &src[4 * i..4 * i + 4];
            let d = &dst[4 * i..4 * i + 4];
            let (r, g) = match s {
                0 => (
                    f32::from_bits(CLEAR_BITS[k][0]),
                    f32::from_bits(CLEAR_BITS[k][1]),
                ),
                1 | 4 | 5 | 6 => (c[0], c[1]),
                2 => match k {
                    0 => (c[0], d[1]),
                    1 => (d[0], c[1]),
                    2 | 3 | 5 => (d[0], d[1]),
                    _ => (c[0], c[1]),
                },
                3 => match k {
                    0 => (
                        c[0] * c[3] + d[0] * (1.0 - c[3]),
                        c[1] * c[3] + d[1] * (1.0 - c[3]),
                    ),
                    1 => (c[0], c[1]),
                    _ => (d[0], d[1]),
                },
                _ => unreachable!(),
            };
            let p = pack(r, g);
            words.push(p);
            if s == 5 {
                words.push(pack(c[2], c[3]));
            }
            if s == 6 {
                let r = (SAMPLE_X[i % 16].clamp(0.0, 15.0) + 1.0) / 16.0;
                let g = 0.25 + SAMPLE_Y[i / 16].clamp(0.0, 3.0) / 8.0;
                for _ in 0..2 {
                    aux.extend_from_slice(&[r.to_bits(), g.to_bits(), 0, 0x3f800000]);
                }
            }
            if s == 4 {
                for _ in 0..2 {
                    aux.extend_from_slice(&[
                        widen(p as u16),
                        widen((p >> 16) as u16),
                        0,
                        0x3f800000,
                    ]);
                }
            }
        }
    }
    (words, aux)
}
struct Check {
    mismatches: usize,
    aux_mismatches: usize,
    lengths: bool,
    mixed_pair_matches: bool,
    lod_pair_matches: bool,
    details: Vec<String>,
}
impl Check {
    fn pass(&self) -> bool {
        self.lengths
            && self.mixed_pair_matches
            && self.lod_pair_matches
            && self.mismatches == 0
            && self.aux_mismatches == 0
    }
    fn json(&self) -> String {
        obj(&[
            ("pass", self.pass().to_string()),
            (
                "lod_sampler_pair_matches",
                self.lod_pair_matches.to_string(),
            ),
            ("lengths_match", self.lengths.to_string()),
            (
                "mixed_visible_pair_matches",
                self.mixed_pair_matches.to_string(),
            ),
            ("storage_component_mismatches", self.mismatches.to_string()),
            (
                "sample_component_mismatches",
                self.aux_mismatches.to_string(),
            ),
            ("first_mismatches", array(self.details.clone())),
        ])
    }
}
fn check(s: u32, words: &[u32], aux: &[u32]) -> Check {
    let (w, a) = expected(s);
    let mut c = Check {
        mismatches: 0,
        aux_mismatches: 0,
        lengths: w.len() == words.len() && a.len() == aux.len(),
        mixed_pair_matches: s != 5
            || (words.len() == PIXELS * 4 && words[..PIXELS * 2] == words[PIXELS * 2..]),
        lod_pair_matches: s != 6
            || (aux.len() == PIXELS * 8 && aux.chunks_exact(8).all(|p| p[..4] == p[4..])),
        details: Vec::new(),
    };
    for (i, (&e, &v)) in w.iter().zip(words).enumerate() {
        for ch in 0..2 {
            let eh = (e >> (ch * 16)) as u16;
            let vh = (v >> (ch * 16)) as u16;
            if !component_matches(eh, vh) {
                c.mismatches += 1;
                if c.details.len() < 12 {
                    c.details.push(obj(&[
                        ("word", i.to_string()),
                        ("channel", ch.to_string()),
                        ("expected", hex(eh as u32)),
                        ("actual", hex(vh as u32)),
                    ]));
                }
            }
        }
    }
    for (i, (&e, &v)) in a.iter().zip(aux).enumerate() {
        if e != v {
            c.aux_mismatches += 1;
            if c.details.len() < 12 {
                c.details.push(obj(&[
                    ("aux_word", i.to_string()),
                    ("expected", hex(e)),
                    ("actual", hex(v)),
                ]));
            }
        }
    }
    c
}
#[repr(C)]
#[derive(Default)]
struct Evidence {
    abi_version: u32,
    command_status: u32,
    completed: u32,
    no_error: u32,
    readback_copied: u32,
    pixel_format: u32,
    usage: u32,
    storage_mode: u32,
    texture_count: u32,
    pipelines_created: u32,
    device_registry_id: u64,
}
impl Evidence {
    fn valid(&self, s: u32) -> bool {
        self.abi_version == 1
            && self.command_status == 4
            && self.completed == 1
            && self.no_error == 1
            && self.readback_copied == 1
            && self.pixel_format == if s == 5 { 115 } else { 65 }
            && self.usage == if s == 4 || s == 6 { 5 } else { 4 }
            && self.storage_mode == 2
            && self.texture_count as usize == count(s)
            && self.pipelines_created
                == match s {
                    2 => 8,
                    3 => 5,
                    4 | 5 | 6 => 2,
                    _ => 1,
                }
            && self.device_registry_id != 0
    }
}
#[cfg(all(rg16_native, target_os = "macos"))]
extern "C" {
    fn rg16_native(
        s: u32,
        v: u32,
        source: *const std::ffi::c_char,
        src: *const f32,
        dst: *const f32,
        n: usize,
        clears: *const f64,
        pairs: usize,
        words: *mut u32,
        nwords: usize,
        aux: *mut f32,
        naux: usize,
        e: *mut Evidence,
        json: *mut std::ffi::c_char,
        cap: usize,
    ) -> i32;
}
fn native(s: u32, v: u32, words: &mut [u32], aux: &mut [u32]) -> (i32, Evidence, String) {
    #[cfg(all(rg16_native, target_os = "macos"))]
    {
        let (src, dst) = colors(s);
        let cs = clears();
        let source = std::ffi::CString::new(SOURCE).unwrap();
        let mut e = Evidence::default();
        let mut json = vec![0u8; 16384];
        let rc = unsafe {
            rg16_native(
                s,
                v,
                source.as_ptr(),
                src.as_ptr(),
                dst.as_ptr(),
                src.len(),
                cs.as_ptr(),
                cs.len() / 2,
                words.as_mut_ptr(),
                words.len(),
                aux.as_mut_ptr().cast(),
                aux.len(),
                &mut e,
                json.as_mut_ptr().cast(),
                json.len(),
            )
        };
        let end = json.iter().position(|&b| b == 0).unwrap_or(json.len());
        let text = String::from_utf8_lossy(&json[..end]).into_owned();
        (
            rc,
            e,
            if text.starts_with('{') {
                text
            } else {
                obj(&[("status", quote("missing_native_json"))])
            },
        )
    }
    #[cfg(not(all(rg16_native, target_os = "macos")))]
    {
        let _ = (s, v, words, aux, SOURCE);
        (
            23,
            Evidence::default(),
            obj(&[("status", quote("native_metal_unavailable_no_cpu_fallback"))]),
        )
    }
}
fn prepare(out: &Path) -> io::Result<()> {
    fs::create_dir(out)?;
    write_json(out.join("sources.json"), &source_receipts())?;
    for s in 0..7 {
        let (src, dst) = colors(s);
        let (w, a) = expected(s);
        let id = name(s);
        fs::write(
            out.join(format!("{id}.src.bin")),
            bytes(&src.iter().map(|x| x.to_bits()).collect::<Vec<_>>()),
        )?;
        fs::write(
            out.join(format!("{id}.dst.bin")),
            bytes(&dst.iter().map(|x| x.to_bits()).collect::<Vec<_>>()),
        )?;
        fs::write(out.join(format!("{id}.expected.words.bin")), bytes(&w))?;
        fs::write(out.join(format!("{id}.expected.aux.bin")), bytes(&a))?;
    }
    fs::write(
        out.join("clear.input.f64le.bin"),
        clears()
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect::<Vec<_>>(),
    )?;
    write_json(out.join("corpus.json"),&obj(&[("schema","1".into()),("edge_f32_bits",array(EDGES.iter().map(|&b|hex(b)))),
        ("clear_f32_pairs",array(CLEAR_BITS.iter().map(|p|array(p.iter().map(|&b|hex(b)))))),
        ("mask_order",quote("R,G,B,A,RGB,None,All")),("blend_order",quote("srcAlpha/oneMinusSrcAlpha,dstAlpha/zero,oneMinusDstAlpha/one,zero/dstAlpha; all use RGB mask")),
        ("expected_scope",quote("Independent RTE candidate; binary16 NaN accepts NaN or signed zero. Clear inputs are exact f32 values promoted to double. No native execution implied."))]))
}
fn child(s: u32, v: u32, out: &Path, id: &str) -> io::Result<i32> {
    if count(s) == 0 || v > 1 {
        return Ok(64);
    }
    let mut w = vec![0xa5a5a5a5; count(s) * PIXELS * if s == 5 { 2 } else { 1 }];
    let mut a = vec![0xa5a5a5a5; if s == 4 || s == 6 { PIXELS * 8 } else { 0 }];
    let (rc, e, metadata) = native(s, v, &mut w, &mut a);
    write_json(out.join(format!("{id}.native.json")), &metadata)?;
    let valid = rc == 0 && e.valid(s);
    let c = check(s, &w, &a);
    // A negative is rejected only after a valid native command/readback, with
    // a targeted mismatch. Compilation/device errors never count as rejection.
    let targeted = if s == 4 || s == 6 {
        c.aux_mismatches > 0
    } else {
        c.mismatches > 0
    };
    let rejected = valid && v == 1 && c.lengths && targeted;
    let accepted = valid && v == 0 && c.pass();
    let exit = if accepted {
        0
    } else if rejected {
        10
    } else {
        2
    };
    if valid {
        fs::write(out.join(format!("{id}.gpu.words.bin")), bytes(&w))?;
        fs::write(out.join(format!("{id}.gpu.aux.bin")), bytes(&a))?;
        fs::write(
            out.join(format!("{id}.device.bin")),
            e.device_registry_id.to_le_bytes(),
        )?;
    }
    write_json(
        out.join(format!("{id}.receipt.json")),
        &obj(&[
            ("schema", "1".into()),
            ("scenario", s.to_string()),
            ("variant", v.to_string()),
            ("native_return_code", rc.to_string()),
            ("gpu_readback_valid", valid.to_string()),
            ("candidate_contract_passed", accepted.to_string()),
            ("negative_rejected", rejected.to_string()),
            ("exit_code", exit.to_string()),
            ("checks", c.json()),
        ]),
    )?;
    Ok(exit)
}
fn observe(out: &Path) -> io::Result<i32> {
    prepare(out)?;
    let exe = env::current_exe()?;
    let mut records = Vec::new();
    let mut qualified = true;
    let mut devices = Vec::new();
    for s in 0..7 {
        for v in 0..2 {
            let id = format!("{}-v{v}", name(s));
            let started = Instant::now();
            let mut p = Command::new(&exe)
                .args(["--child", &s.to_string(), &v.to_string()])
                .arg(out)
                .arg(&id)
                .stdout(Stdio::from(fs::File::create(
                    out.join(format!("{id}.stdout.txt")),
                )?))
                .stderr(Stdio::from(fs::File::create(
                    out.join(format!("{id}.stderr.txt")),
                )?))
                .spawn()?;
            let mut timeout = false;
            let status = loop {
                if let Some(x) = p.try_wait()? {
                    break x;
                }
                if started.elapsed() > Duration::from_secs(45) {
                    timeout = true;
                    p.kill()?;
                    break p.wait()?;
                }
                thread::sleep(Duration::from_millis(25));
            };
            let expected = if v == 0 { 0 } else { 10 };
            let pass = !timeout && status.code() == Some(expected);
            qualified &= pass;
            if pass {
                let data = fs::read(out.join(format!("{id}.device.bin")))?;
                let d: [u8; 8] = data
                    .try_into()
                    .map_err(|_| io::Error::other("bad device identity"))?;
                devices.push(u64::from_le_bytes(d));
            }
            records.push(obj(&[
                ("case", quote(&id)),
                ("expected_exit", expected.to_string()),
                (
                    "actual_exit",
                    status.code().map_or("null".into(), |c| c.to_string()),
                ),
                ("timed_out", timeout.to_string()),
                ("pass", pass.to_string()),
            ]));
        }
    }
    let same = devices.len() == 14 && devices[0] != 0 && devices.iter().all(|d| *d == devices[0]);
    qualified &= same;
    write_json(out.join("summary.json"),&obj(&[("probe_version",quote(VERSION)),("backend",quote("native_metal_required")),("gpu_probe_qualified",qualified.to_string()),
        ("same_device_all_cases",same.to_string()),("processes",array(records)),("scope",quote("Generic API/storage and paired mixed-attachment observation on the recorded device and OS. RTE candidate is strict for finite and infinite values; NaN payload/sign excluded. No application or other OS build equivalence."))]))?;
    Ok(if qualified { 0 } else { 2 })
}
fn read_words(path: &Path) -> io::Result<Vec<u32>> {
    let m = fs::metadata(path)?;
    if m.len() > 16384 || m.len() % 4 != 0 {
        return Err(io::Error::other("invalid bounded word file"));
    }
    Ok(fs::read(path)?
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect())
}
fn version(program: &str, args: &[&str]) -> String {
    match Command::new(program).args(args).output() {
        Ok(o) => obj(&[
            (
                "exit_code",
                o.status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or("null".into()),
            ),
            (
                "stdout",
                quote(
                    &String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter(|s| !s.starts_with("InstalledDir:"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ),
        ]),
        Err(_) => obj(&[("status", quote("unavailable"))]),
    }
}
fn seal(out: &Path, exit_code: i32) -> io::Result<i32> {
    fn walk(
        dir: &Path,
        root: &Path,
        items: &mut Vec<(String, Vec<u8>)>,
        total: &mut usize,
    ) -> io::Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let ty = e.file_type()?;
            let name = e.file_name();
            if name == ".build" || name == "run-receipt.json" {
                continue;
            }
            if ty.is_symlink() {
                return Err(io::Error::other("symlink not allowed in receipt"));
            }
            if ty.is_dir() {
                walk(&e.path(), root, items, total)?;
            } else if ty.is_file() {
                let size = e.metadata()?.len();
                if size > 4 * 1024 * 1024
                    || *total as u64 + size > 32 * 1024 * 1024
                    || items.len() >= 192
                {
                    return Err(io::Error::other("bounded receipt budget exceeded"));
                }
                let data = fs::read(e.path())?;
                *total += data.len();
                items.push((
                    e.path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    data,
                ));
            }
        }
        Ok(())
    }
    let mut items = Vec::new();
    let mut total = 0;
    walk(out, out, &mut items, &mut total)?;
    items.sort_by(|a, b| a.0.cmp(&b.0));
    write_json(out.join("run-receipt.json"),&obj(&[("schema","1".into()),("probe_version",quote(VERSION)),
        ("command_exit_code",exit_code.to_string()),("os",quote(env::consts::OS)),("arch",quote(env::consts::ARCH)),
        ("rustc",version("rustc",&["--version","--verbose"])),("xcode",version("xcodebuild",&["-version"])),
        ("sdk",version("xcrun",&["--sdk","macosx","--show-sdk-version"])),("sdk_build",version("xcrun",&["--sdk","macosx","--show-sdk-build-version"])),
        ("clang",version("xcrun",&["--sdk","macosx","clang","--version"])),
        ("metal_compiler",version("xcrun",&["--sdk","macosx","metal","--version"])),("source_files",source_receipts()),
        ("artifact_count",items.len().to_string()),("total_byte_length",total.to_string()),
        ("files",array(items.iter().map(|(n,d)|digest(n,d)))),
        ("scope",quote("Integrity receipt only; consult native, process and summary receipts for device and command qualification."))]))?;
    Ok(0)
}
fn entry() -> io::Result<i32> {
    let a: Vec<_> = env::args().collect();
    let parse = |x: &str| {
        x.parse::<u32>()
            .map_err(|_| io::Error::other("invalid argument"))
    };
    if a.len() == 6 && a[1] == "--child" {
        return child(parse(&a[2])?, parse(&a[3])?, Path::new(&a[4]), &a[5]);
    }
    if a.len() == 4 && a[1] == "seal" {
        return seal(
            Path::new(&a[2]),
            a[3].parse()
                .map_err(|_| io::Error::other("invalid exit code"))?,
        );
    }
    if a.len() == 5 && a[1] == "compare" {
        let s = parse(&a[2])?;
        if count(s) == 0 {
            return Ok(64);
        }
        let w = read_words(Path::new(&a[3]))?;
        let aux = read_words(Path::new(&a[4]))?;
        let c = check(s, &w, &aux);
        println!("{}", c.json());
        return Ok(if c.pass() { 0 } else { 2 });
    }
    if a.len() == 3 {
        match a[1].as_str() {
            "expected" => {
                prepare(Path::new(&a[2]))?;
                return Ok(0);
            }
            "observe" => return observe(Path::new(&a[2])),
            "reference-negative" => {
                prepare(Path::new(&a[2]))?;
                let mut all_rejected = true;
                let mut records = Vec::new();
                for s in 0..7 {
                    let (mut w, mut aux) = expected(s);
                    if s == 4 || s == 6 {
                        aux[3] = 0;
                    } else {
                        w[0] ^= 1;
                    }
                    let c = check(s, &w, &aux);
                    all_rejected &= !c.pass();
                    records.push(c.json());
                    fs::write(
                        Path::new(&a[2]).join(format!("{}.wrong.words.bin", name(s))),
                        bytes(&w),
                    )?;
                    fs::write(
                        Path::new(&a[2]).join(format!("{}.wrong.aux.bin", name(s))),
                        bytes(&aux),
                    )?;
                }
                write_json(
                    Path::new(&a[2]).join("summary.json"),
                    &obj(&[
                        ("backend", quote("portable_comparator_only")),
                        ("gpu_executed", "false".into()),
                        ("all_corruptions_rejected", all_rejected.to_string()),
                        ("checks", array(records)),
                    ]),
                )?;
                return Ok(if all_rejected { 2 } else { 0 });
            }
            _ => {}
        }
    }
    eprintln!("usage: rg16-probe expected|observe|reference-negative NEW_DIRECTORY; compare SCENARIO WORDS AUX; seal DIRECTORY EXIT_CODE");
    Ok(64)
}
fn main() {
    std::process::exit(match entry() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("probe error: {e}");
            1
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_half_edges() {
        for (b, h) in [
            (0x00000000, 0x0000),
            (0x80000000, 0x8000),
            (0x3f800000, 0x3c00),
            (0xbf800000, 0xbc00),
            (0x3f800fff, 0x3c00),
            (0x3f801000, 0x3c00),
            (0x3f801001, 0x3c01),
            (0x3f802fff, 0x3c01),
            (0x3f803000, 0x3c02),
            (0x3f803001, 0x3c02),
            (0xbf801000, 0xbc00),
            (0xbf803000, 0xbc02),
            (0x33000000, 0x0000),
            (0x33000001, 0x0001),
            (0x33800000, 0x0001),
            (0x33c00000, 0x0002),
            (0xb3000000, 0x8000),
            (0xb3800000, 0x8001),
            (0x387fc000, 0x03ff),
            (0x387fe000, 0x0400),
            (0x38800000, 0x0400),
            (0x477fe000, 0x7bff),
            (0x477fefff, 0x7bff),
            (0x477ff000, 0x7c00),
            (0xc77ff000, 0xfc00),
            (0x7f800000, 0x7c00),
            (0xff800000, 0xfc00),
        ] {
            assert_eq!(half(b), h, "{b:08x}");
        }
    }
    #[test]
    fn exhaustive_finite_roundtrip() {
        for h in 0..=u16::MAX {
            if !nan16(h) {
                assert_eq!(half(widen(h)), h);
            }
        }
    }
    #[test]
    fn literal_widen() {
        for (h, b) in [
            (0x0001, 0x33800000),
            (0x03ff, 0x387fc000),
            (0x0400, 0x38800000),
            (0x7bff, 0x477fe000),
            (0x8000, 0x80000000),
        ] {
            assert_eq!(widen(h), b);
        }
    }
    #[test]
    fn independent_case_anchors() {
        // First pixel: S=(1/16,1/4,A=0), D=(1/8,3/4,A=0).
        let (w, _) = expected(0);
        assert_eq!(w[0], 0xbe003400);
        let (w, _) = expected(2);
        assert_eq!(w[0], 0x3a002c00);
        assert_eq!(w[PIXELS], 0x34003000);
        for k in [2, 3, 5] {
            assert_eq!(w[k * PIXELS], 0x3a003000);
        }
        for k in [4, 6] {
            assert_eq!(w[k * PIXELS], 0x34002c00);
        }
        let (w, _) = expected(3);
        assert_eq!(w[0], 0x3a003000);
        assert_eq!(w[PIXELS], 0x34002c00);
        assert_eq!(w[2 * PIXELS], 0x3a003000);
        assert_eq!(w[3 * PIXELS], 0x3a003000);
        // Pixel 1: source=(1/8,1/4,A=1/4), dest=(1/8,23/32).
        assert_eq!(w[1], 0x38d03000); // G = 77/128 = 0.6015625.
        let (_, a) = expected(4);
        assert_eq!(
            &a[..8],
            &[0x3d800000, 0x3e800000, 0, 0x3f800000, 0x3d800000, 0x3e800000, 0, 0x3f800000]
        );
    }
    #[test]
    fn comparator_rejects_corruption_and_truncation() {
        for s in 0..7 {
            let (w, a) = expected(s);
            assert!(check(s, &w, &a).pass());
            assert!(!check(s, &w[1..], &a).pass());
            let mut bad = w.clone();
            bad[0] ^= 1;
            assert!(!check(s, &bad, &a).pass());
            if s == 4 || s == 6 {
                for channel in [0, 1, 2, 3, 4, 5, 6, 7] {
                    let mut bad = a.clone();
                    bad[channel] ^= 1;
                    assert!(!check(s, &w, &bad).pass());
                }
            }
        }
    }
    #[test]
    fn nan_policy_bounded() {
        assert!(component_matches(0x7e00, 0));
        assert!(component_matches(0x7e00, 0x8000));
        assert!(component_matches(0x7e00, 0x7fff));
        assert!(!component_matches(0x7e00, 0x7c00));
    }
    #[test]
    fn sha_known() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn evidence_abi_and_failure_closed() {
        assert_eq!(std::mem::size_of::<Evidence>(), 48);
        for s in 0..7 {
            assert!(!Evidence::default().valid(s));
        }
    }
    #[test]
    fn mixed_visible_rgba_and_discard_exclusion() {
        let (w, a) = expected(5);
        assert_eq!(w.len(), 256);
        assert!(a.is_empty());
        assert_eq!(&w[..2], &[0x34002c00, 0x0000c500]);
        assert_eq!(&w[..128], &w[128..]);
        assert!(check(5, &w, &a).pass());
        let mut bad = w.clone();
        bad[128] ^= 1;
        assert!(!check(5, &bad, &a).mixed_pair_matches);
        assert!(!check(5, &bad, &a).pass());
        // No output allocation represents either discarded attachment.
        assert_eq!(count(5) * PIXELS * 8, w.len() * 4);
    }
    #[test]
    fn lod_average_pair_reference_and_negative() {
        let (w, a) = expected(6);
        assert_eq!(w.len(), 64);
        assert_eq!(a.len(), 512);
        assert_eq!(
            &a[..8],
            &[0x3d800000, 0x3e800000, 0, 0x3f800000, 0x3d800000, 0x3e800000, 0, 0x3f800000]
        );
        assert_eq!(a[8], 0x3da00000); // x=1/4: (1+1/4)/16=5/64.
        assert!(check(6, &w, &a).pass());
        let mut b = a.clone();
        b[4] = 0;
        assert!(!check(6, &w, &b).lod_pair_matches);
        assert!(!check(6, &w, &b).pass());
        let (_, params) = colors(6);
        assert_eq!(&params[..2], &[-0.015625, -0.0625]);
    }
}
