use std::{
    env, fs, io,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const VERSION: &str = "generic-half-boundary-1";
const SOURCE: &str = include_str!("probe.metal");
const SOURCES: [(&str, &[u8]); 5] = [
    ("scripts/boundary/host.rs", include_bytes!("host.rs")),
    ("scripts/boundary/bridge.m", include_bytes!("bridge.m")),
    (
        "scripts/boundary/probe.metal",
        include_bytes!("probe.metal"),
    ),
    ("scripts/boundary/run.sh", include_bytes!("run.sh")),
    (
        ".github/workflows/boundary.yml",
        include_bytes!("../../.github/workflows/boundary.yml"),
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

fn half_bytes(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
// Independent nearest-neighbor reference; no platform half instructions.
struct Grid(Vec<f64>);
fn widen(h: u16) -> f64 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = (h >> 10) & 31;
    let f = h & 1023;
    sign * match e {
        0 => f as f64 * 2f64.powi(-24),
        31 => {
            if f == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1024 + f) as f64 * 2f64.powi(e as i32 - 25),
    }
}
impl Grid {
    fn new() -> Self {
        Self((0..=0x7bffu16).map(widen).collect())
    }
    fn convert(&self, b: u32, rne: bool) -> u16 {
        let x = f32::from_bits(b) as f64;
        let sign = if b >> 31 == 1 { 0x8000 } else { 0 };
        if x.is_nan() {
            return sign | 0x7e00;
        }
        if x.is_infinite() {
            return sign | 0x7c00;
        }
        let a = x.abs();
        if a == 0.0 {
            return sign;
        }
        let lo = self.0.partition_point(|&v| v <= a) - 1;
        if !rne || self.0[lo] == a {
            return sign | lo as u16;
        }
        let hi = lo + 1;
        let value = if hi == 0x7c00 {
            if a >= 65520.0 {
                0x7c00
            } else {
                0x7bff
            }
        } else {
            let dl = a - self.0[lo];
            let dh = self.0[hi] - a;
            if dl < dh || (dl == dh && lo & 1 == 0) {
                lo
            } else {
                hi
            }
        };
        sign | value as u16
    }
}
const POS: [u32; 32] = [
    0, 0x32ffffff, 0x33000000, 0x33000001, 0x33800000, 0x33bfffff, 0x33c00000, 0x33c00001,
    0x387fbfff, 0x387fc000, 0x387fdfff, 0x387fe000, 0x387fe001, 0x38800000, 0x3e800000, 0x3f000000,
    0x3f800000, 0x3f800fff, 0x3f801000, 0x3f801001, 0x3f802fff, 0x3f803000, 0x3f803001, 0x3fc00000,
    0x477fe000, 0x477fefff, 0x477ff000, 0x477ff001, 0x47800000, 0x501502f9, 0x7f7fffff, 0x3f000001,
];
fn corpus(group: u32) -> Vec<u32> {
    let mut v = Vec::new();
    if group == 0 {
        let pool: Vec<_> = POS.iter().flat_map(|&p| [p, p | 0x80000000]).collect();
        for i in 0..64 {
            for c in 0..4 {
                v.push(pool[(i + 17 * c) % 64]);
            }
        }
    } else {
        let p = [
            0x7f800000, 0xff800000, 0x7fc00001, 0xffc12345, 0x3f800000, 0xbf800000, 0x3e800000,
            0xbe800000,
        ];
        for i in 0..8 {
            for c in 0..4 {
                v.push(p[(i + 3 * c) % 8]);
            }
        }
    }
    v[..4].copy_from_slice(&[0x3e800000, 0xbf000000, 0x3f800000, 0x40000000]);
    v
}
fn n(group: u32) -> usize {
    if group == 0 {
        64
    } else {
        8
    }
}
fn id(ch: u32, route: u32, group: u32, v: u32) -> String {
    format!("c{ch}-r{route}-g{group}-v{v}")
}
fn nan(h: u16) -> bool {
    h & 0x7c00 == 0x7c00 && h & 1023 != 0
}
#[derive(Default)]
struct Profile {
    non_nan: usize,
    mismatch: usize,
    nan_inputs: usize,
    nan_outputs: usize,
    nan_zeros: usize,
    nan_other: usize,
}
impl Profile {
    fn json(&self) -> String {
        obj(&[
            ("non_nan_components", self.non_nan.to_string()),
            ("non_nan_mismatches", self.mismatch.to_string()),
            ("nan_inputs", self.nan_inputs.to_string()),
            ("nan_outputs", self.nan_outputs.to_string()),
            ("nan_zero_outputs", self.nan_zeros.to_string()),
            ("nan_other_outputs", self.nan_other.to_string()),
        ])
    }
}
fn profile(g: &Grid, src: &[u32], got: &[u16], rne: bool) -> Profile {
    let mut p = Profile::default();
    for (&b, &h) in src.iter().zip(got) {
        if f32::from_bits(b).is_nan() {
            p.nan_inputs += 1;
            if nan(h) {
                p.nan_outputs += 1;
            } else if h & 0x7fff == 0 {
                p.nan_zeros += 1;
            } else {
                p.nan_other += 1;
            }
        } else {
            p.non_nan += 1;
            p.mismatch += usize::from(g.convert(b, rne) != h);
        }
    }
    p
}
#[derive(Clone)]
struct Observation {
    pixels: Vec<u16>,
    echo: Vec<u32>,
    casts: Vec<u16>,
}
impl Observation {
    fn sentinel(ch: u32, group: u32) -> Self {
        Self {
            pixels: vec![0xa5a5; n(group) * ch as usize],
            echo: vec![0xa5a5a5a5; n(group) * 4],
            casts: vec![0xa5a5; n(group) * 4],
        }
    }
    fn save(&self, out: &Path, key: &str) -> io::Result<()> {
        fs::write(
            out.join(format!("{key}.pixels.bin")),
            half_bytes(&self.pixels),
        )?;
        fs::write(out.join(format!("{key}.echo.bin")), bytes(&self.echo))?;
        fs::write(
            out.join(format!("{key}.casts.bin")),
            half_bytes(&self.casts),
        )
    }
    fn load(out: &Path, key: &str) -> io::Result<Self> {
        let get = |suffix: &str| -> io::Result<Vec<u8>> {
            let p = out.join(format!("{key}.{suffix}.bin"));
            if fs::metadata(&p)?.len() > 8192 {
                return Err(io::Error::other("oversized observation"));
            }
            fs::read(p)
        };
        let p = get("pixels")?;
        let e = get("echo")?;
        let c = get("casts")?;
        if p.len() % 2 != 0 || c.len() % 2 != 0 || e.len() % 4 != 0 {
            return Err(io::Error::other("invalid observation length"));
        }
        Ok(Self {
            pixels: p
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
                .collect(),
            echo: e
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .collect(),
            casts: c
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
                .collect(),
        })
    }
    fn shape(&self, ch: u32, group: u32) -> bool {
        self.pixels.len() == n(group) * ch as usize
            && self.echo.len() == n(group) * 4
            && self.casts.len() == n(group) * 4
    }
}
fn stored_input(src: &[u32], ch: u32) -> Vec<u32> {
    src.chunks_exact(4)
        .flat_map(|p| p[..ch as usize].iter().copied())
        .collect()
}
fn stored_cast(casts: &[u16], ch: u32) -> Vec<u16> {
    casts
        .chunks_exact(4)
        .flat_map(|p| p[..ch as usize].iter().copied())
        .collect()
}
struct Check {
    pass: bool,
    json: String,
}
fn check(g: &Grid, o: &Observation, ch: u32, route: u32, group: u32) -> Check {
    let src = corpus(group);
    let shape = o.shape(ch, group);
    let s = stored_input(&src, ch);
    let c = stored_cast(&o.casts, ch);
    let echo_mismatch = src.iter().zip(&o.echo).filter(|(a, b)| a != b).count();
    let r = profile(g, &s, &o.pixels, true);
    let z = profile(g, &s, &o.pixels, false);
    let cr = profile(g, &src, &o.casts, true);
    let cz = profile(g, &src, &o.casts, false);
    let export_vs_cast = s
        .iter()
        .zip(o.pixels.iter().zip(&c))
        .filter(|(b, (a, h))| !f32::from_bits(**b).is_nan() && a != h)
        .count();
    let nan_export_bits = s
        .iter()
        .zip(o.pixels.iter().zip(&c))
        .filter(|(b, (a, h))| f32::from_bits(**b).is_nan() && a != h)
        .count();
    let anchor = o.pixels.first() == Some(&0x3400);
    let cast_anchor = o.casts.first() == Some(&0x3400);
    // Only the explicit arithmetic cast and preservation of its representable
    // results are contracts here. Clear/float-export RNE and RTZ stay hypotheses.
    let pass = shape
        && echo_mismatch == 0
        && anchor
        && cast_anchor
        && cr.mismatch == 0
        && (route < 2 || export_vs_cast == 0);
    Check {
        pass,
        json: obj(&[
            ("shape_valid", shape.to_string()),
            ("echo_mismatches", echo_mismatch.to_string()),
            ("positive_anchor", anchor.to_string()),
            (
                "cast_rne_contract_non_nan_pass",
                (cr.mismatch == 0 && shape).to_string(),
            ),
            ("original_input_rne", r.json()),
            ("original_input_rtz", z.json()),
            ("explicit_cast_rne", cr.json()),
            ("explicit_cast_rtz", cz.json()),
            (
                "storage_vs_recorded_cast_non_nan_differences",
                export_vs_cast.to_string(),
            ),
            (
                "storage_vs_recorded_cast_nan_bit_differences",
                nan_export_bits.to_string(),
            ),
            ("required_checks_pass", pass.to_string()),
            ("export_rounding_hypothesis_selected", "false".into()),
        ]),
    }
}
fn exact_negative(pos: &Observation, neg: &Observation, ch: u32, group: u32) -> bool {
    pos.shape(ch, group)
        && neg.shape(ch, group)
        && pos.pixels[0] == 0x3400
        && neg.pixels[0] == 0xb400
        && pos.pixels[1..] == neg.pixels[1..]
        && pos.echo == neg.echo
        && pos.casts == neg.casts
        && pos.echo == corpus(group)
}
#[repr(C)]
#[derive(Default)]
struct Evidence {
    abi: u32,
    status: u32,
    completed: u32,
    no_error: u32,
    copied: u32,
    format: u32,
    route: u32,
    count: u32,
    variant: u32,
    sidecar_stage: u32,
    device_id: u64,
}
impl Evidence {
    fn valid(&self, ch: u32, route: u32, group: u32, v: u32) -> bool {
        self.abi == 1
            && self.status == 4
            && self.completed == 1
            && self.no_error == 1
            && self.copied == 1
            && self.format == if ch == 2 { 65 } else { 115 }
            && self.route == route
            && self.count == n(group) as u32
            && self.variant == v
            && self.sidecar_stage == if route == 0 { 1 } else { 2 }
            && self.device_id != 0
    }
}
#[cfg(all(boundary_native, target_os = "macos"))]
extern "C" {
    fn boundary_native(
        ch: u32,
        route: u32,
        v: u32,
        source: *const std::ffi::c_char,
        input: *const u32,
        count: usize,
        pixels: *mut u16,
        echo: *mut u32,
        casts: *mut u16,
        e: *mut Evidence,
        json: *mut std::ffi::c_char,
        cap: usize,
    ) -> i32;
}
fn native(ch: u32, route: u32, group: u32, v: u32, o: &mut Observation) -> (i32, Evidence, String) {
    #[cfg(all(boundary_native, target_os = "macos"))]
    {
        let src = corpus(group);
        let code = std::ffi::CString::new(SOURCE).unwrap();
        let mut e = Evidence::default();
        let mut j = vec![0u8; 16384];
        let rc = unsafe {
            boundary_native(
                ch,
                route,
                v,
                code.as_ptr(),
                src.as_ptr(),
                n(group),
                o.pixels.as_mut_ptr(),
                o.echo.as_mut_ptr(),
                o.casts.as_mut_ptr(),
                &mut e,
                j.as_mut_ptr().cast(),
                j.len(),
            )
        };
        let end = j.iter().position(|&b| b == 0).unwrap_or(j.len());
        let s = String::from_utf8_lossy(&j[..end]).into_owned();
        (
            rc,
            e,
            if s.starts_with('{') {
                s
            } else {
                "{\"status\":\"missing_native_json\"}".into()
            },
        )
    }
    #[cfg(not(all(boundary_native, target_os = "macos")))]
    {
        let _ = (ch, route, group, v, o, SOURCE);
        (
            23,
            Evidence::default(),
            "{\"status\":\"native_unavailable_no_cpu_fallback\"}".into(),
        )
    }
}
fn prepare(out: &Path) -> io::Result<()> {
    fs::create_dir(out)?;
    write_json(out.join("sources.json"), &source_receipts())?;
    let g = Grid::new();
    for group in 0..2 {
        let input = corpus(group);
        fs::write(
            out.join(format!("group-{group}.input.u32le.bin")),
            bytes(&input),
        )?;
        for (rne, label) in [(true, "rne"), (false, "rtz")] {
            let h: Vec<_> = input.iter().map(|&b| g.convert(b, rne)).collect();
            fs::write(
                out.join(format!("group-{group}.{label}.u16le.bin")),
                half_bytes(&h),
            )?;
        }
    }
    write_json(out.join("corpus.json"),&obj(&[("schema","1".into()),("version",quote(VERSION)),("routes",quote("0=clear,1=float4,2=half4,3=half_widened_float4")),
  ("groups",quote("0=64 finite RGBA inputs,1=8 separately classified finite-anchor/Inf/quiet-NaN inputs; no signaling NaNs")),
  ("negative",quote("only pixel0 R changes from +0.25 to -0.25; all other storage and both sidecars identical")),
  ("default_export_rounding",quote("RNE and RTZ hypotheses both retained; neither chosen by this probe"))]))
}
fn table(g: &Grid, o: &Observation, ch: u32, group: u32) -> String {
    let src = corpus(group);
    let mut t = String::from(
        "pixel,channel,original_f32,echo_f32,explicit_cast_half,rne_half,rtz_half,stored_half\n",
    );
    for i in 0..n(group) {
        for c in 0..4 {
            let b = src[4 * i + c];
            let storage = if c < ch as usize {
                format!("0x{:04x}", o.pixels[i * ch as usize + c])
            } else {
                "absent".into()
            };
            t.push_str(&format!(
                "{i},{c},0x{b:08x},0x{:08x},0x{:04x},0x{:04x},0x{:04x},{storage}\n",
                o.echo[4 * i + c],
                o.casts[4 * i + c],
                g.convert(b, true),
                g.convert(b, false)
            ));
        }
    }
    t
}
fn child(ch: u32, route: u32, group: u32, v: u32, out: &Path) -> io::Result<i32> {
    if ![2, 4].contains(&ch) || route > 3 || group > 1 || v > 1 {
        return Ok(64);
    }
    let key = id(ch, route, group, v);
    let mut o = Observation::sentinel(ch, group);
    let (rc, e, meta) = native(ch, route, group, v, &mut o);
    write_json(out.join(format!("{key}.native.json")), &meta)?;
    let valid = rc == 0 && e.valid(ch, route, group, v);
    let g = Grid::new();
    let c = check(&g, &o, ch, route, group);
    if valid {
        fs::write(
            out.join(format!("{key}.components.csv")),
            table(&g, &o, ch, group),
        )?;
        o.save(out, &key)?;
        fs::write(
            out.join(format!("{key}.device.bin")),
            e.device_id.to_le_bytes(),
        )?;
    }
    let exact = if valid && v == 1 {
        let pk = id(ch, route, group, 0);
        match (
            Observation::load(out, &pk),
            fs::read(out.join(format!("{pk}.device.bin"))),
        ) {
            (Ok(p), Ok(d)) => d == e.device_id.to_le_bytes() && exact_negative(&p, &o, ch, group),
            _ => false,
        }
    } else {
        false
    };
    let exit = if valid && v == 0 && c.pass {
        0
    } else if valid && v == 1 && exact && !c.pass {
        10
    } else {
        2
    };
    write_json(out.join(format!("{key}.check.json")),&obj(&[("native_return_code",rc.to_string()),("native_valid",valid.to_string()),("variant",v.to_string()),
  ("exit_code",exit.to_string()),("exact_negative_difference",exact.to_string()),("positive_contract_checks",c.json),
  ("scope",quote("Exit checks execution, original echoes, explicit non-NaN RNE cast, prequantized export preservation, and exact controls. It does not select default clear/float-export rounding."))]))?;
    Ok(exit)
}
fn observe(out: &Path) -> io::Result<i32> {
    prepare(out)?;
    let exe = env::current_exe()?;
    let mut records = Vec::new();
    let mut ids = Vec::new();
    let mut passed = [true; 2];
    for group in 0..2 {
        for ch in [2, 4] {
            for route in 0..4 {
                for v in 0..2 {
                    let key = id(ch, route, group, v);
                    let start = Instant::now();
                    let mut p = Command::new(&exe)
                        .args([
                            "--child",
                            &ch.to_string(),
                            &route.to_string(),
                            &group.to_string(),
                            &v.to_string(),
                        ])
                        .arg(out)
                        .stdout(Stdio::from(fs::File::create(
                            out.join(format!("{key}.stdout.txt")),
                        )?))
                        .stderr(Stdio::from(fs::File::create(
                            out.join(format!("{key}.stderr.txt")),
                        )?))
                        .spawn()?;
                    let mut timeout = false;
                    let status = loop {
                        if let Some(s) = p.try_wait()? {
                            break s;
                        }
                        if start.elapsed() > Duration::from_secs(20) {
                            timeout = true;
                            p.kill()?;
                            break p.wait()?;
                        }
                        thread::sleep(Duration::from_millis(25));
                    };
                    let wanted = if v == 0 { 0 } else { 10 };
                    let ok = !timeout && status.code() == Some(wanted);
                    passed[group as usize] &= ok;
                    // Device observations are collected independently of numeric/check exit code.
                    if let Ok(b) = fs::read(out.join(format!("{key}.device.bin"))) {
                        if b.len() == 8 {
                            ids.push(u64::from_le_bytes(b.try_into().unwrap()));
                        }
                    }
                    records.push(obj(&[
                        ("case", quote(&key)),
                        ("expected_exit", wanted.to_string()),
                        (
                            "actual_exit",
                            status.code().map_or("null".into(), |n| n.to_string()),
                        ),
                        ("timed_out", timeout.to_string()),
                        ("checks_passed", ok.to_string()),
                    ]));
                }
            }
        }
    }
    let same = ids.len() == 32 && ids[0] != 0 && ids.iter().all(|d| *d == ids[0]);
    let mut cross = Vec::new();
    for group in 0..2 {
        for ch in [2, 4] {
            if let Ok(clear) = Observation::load(out, &id(ch, 0, group, 0)) {
                for route in 1..4 {
                    if let Ok(draw) = Observation::load(out, &id(ch, route, group, 0)) {
                        cross.push(obj(&[
                            ("channels", ch.to_string()),
                            ("group", group.to_string()),
                            ("route", route.to_string()),
                            (
                                "echo_equal_compute_clear",
                                (draw.echo == clear.echo).to_string(),
                            ),
                            (
                                "cast_bit_differences_compute_clear",
                                draw.casts
                                    .iter()
                                    .zip(&clear.casts)
                                    .filter(|(a, b)| a != b)
                                    .count()
                                    .to_string(),
                            ),
                        ]));
                    }
                }
            }
        }
    }
    write_json(
        out.join("summary.json"),
        &obj(&[
            ("schema", "1".into()),
            ("version", quote(VERSION)),
            ("finite_contracts_and_controls_pass", passed[0].to_string()),
            (
                "special_non_nan_contracts_and_controls_pass",
                passed[1].to_string(),
            ),
            ("completed_device_records", ids.len().to_string()),
            ("same_device_all_32_observations", same.to_string()),
            ("export_rounding_hypothesis_selected", "false".into()),
            ("numeric_portability_qualified", "false".into()),
            ("cases", array(records)),
            ("cross_route_sidecars", array(cross)),
        ]),
    )?;
    Ok(if passed[0] && passed[1] && same { 0 } else { 2 })
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
                if size > 1024 * 1024
                    || *total as u64 + size > 16 * 1024 * 1024
                    || items.len() >= 384
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
fn simulated(g: &Grid, ch: u32, route: u32, group: u32) -> Observation {
    let src = corpus(group);
    let casts: Vec<_> = src.iter().map(|&b| g.convert(b, true)).collect();
    let pixels = stored_input(&src, ch)
        .iter()
        .map(|&b| g.convert(b, route != 1))
        .collect();
    Observation {
        pixels,
        echo: src,
        casts,
    }
}
fn portable(out: &Path, negative: bool) -> io::Result<i32> {
    prepare(out)?;
    let g = Grid::new();
    let mut records = Vec::new();
    let mut ok = true;
    for group in 0..2 {
        for ch in [2, 4] {
            for route in 0..4 {
                let pos = simulated(&g, ch, route, group);
                let mut bad = pos.clone();
                bad.pixels[0] = 0xb400;
                let selected = if negative { &bad } else { &pos };
                let c = check(&g, selected, ch, route, group);
                let exact = exact_negative(&pos, &bad, ch, group);
                ok &= exact && c.pass != negative;
                records.push(obj(&[
                    ("case", quote(&id(ch, route, group, u32::from(negative)))),
                    ("check", c.json),
                    ("wrong_control_exact", exact.to_string()),
                ]));
            }
        }
    }
    write_json(
        out.join("portable.json"),
        &obj(&[
            ("gpu_executed", "false".into()),
            ("backend", quote("synthetic_checker_test_only")),
            ("tests_valid", ok.to_string()),
            ("negative_mode", negative.to_string()),
            ("cases", array(records)),
        ]),
    )?;
    Ok(if ok {
        if negative {
            2
        } else {
            0
        }
    } else {
        1
    })
}
fn entry() -> io::Result<i32> {
    let a: Vec<_> = env::args().collect();
    let parse = |s: &str| {
        s.parse::<u32>()
            .map_err(|_| io::Error::other("invalid integer"))
    };
    if a.len() == 7 && a[1] == "--child" {
        return child(
            parse(&a[2])?,
            parse(&a[3])?,
            parse(&a[4])?,
            parse(&a[5])?,
            Path::new(&a[6]),
        );
    }
    if a.len() == 4 && a[1] == "seal" {
        return seal(
            Path::new(&a[2]),
            a[3].parse().map_err(|_| io::Error::other("invalid exit"))?,
        );
    }
    if a.len() == 7 && a[1] == "inspect" {
        let (ch, route, group, v) = (parse(&a[2])?, parse(&a[3])?, parse(&a[4])?, parse(&a[5])?);
        if ![2, 4].contains(&ch) || route > 3 || group > 1 || v > 1 {
            return Ok(64);
        }
        let o = Observation::load(Path::new(&a[6]), &id(ch, route, group, v))?;
        let c = check(&Grid::new(), &o, ch, route, group);
        println!("{}", c.json);
        return Ok(if c.pass { 0 } else { 2 });
    }
    if a.len() == 3 {
        match a[1].as_str() {
            "observe" => return observe(Path::new(&a[2])),
            "reference" => return portable(Path::new(&a[2]), false),
            "reference-negative" => return portable(Path::new(&a[2]), true),
            _ => {}
        }
    }
    eprintln!("usage: boundary reference|reference-negative|observe NEW_DIRECTORY; inspect CHANNELS ROUTE GROUP VARIANT DIRECTORY; seal DIRECTORY EXIT");
    Ok(64)
}
fn main() {
    std::process::exit(match entry() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("boundary error: {e}");
            1
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_rounding() {
        let g = Grid::new();
        for (b, r, z) in [
            (0, 0, 0),
            (0x80000000, 0x8000, 0x8000),
            (0x33000001, 1, 0),
            (0xb3000001, 0x8001, 0x8000),
            (0x33c00000, 2, 1),
            (0xb3c00000, 0x8002, 0x8001),
            (0x387fe000, 0x0400, 0x03ff),
            (0x3f801001, 0x3c01, 0x3c00),
            (0xbf803000, 0xbc02, 0xbc01),
            (0x477ff000, 0x7c00, 0x7bff),
            (0x47800000, 0x7c00, 0x7bff),
            (0xc7800000, 0xfc00, 0xfbff),
            (0x7f7fffff, 0x7c00, 0x7bff),
            (0xff7fffff, 0xfc00, 0xfbff),
            (0x7f800000, 0x7c00, 0x7c00),
        ] {
            assert_eq!(g.convert(b, true), r);
            assert_eq!(g.convert(b, false), z);
        }
    }
    #[test]
    fn exhaustive_half_roundtrip_and_midpoints() {
        let g = Grid::new();
        for h in 0..=0x7bffu16 {
            for sign in [0, 0x8000] {
                let b = (widen(h | sign) as f32).to_bits();
                assert_eq!(g.convert(b, true), h | sign);
                assert_eq!(g.convert(b, false), h | sign);
                if h < 0x7bff {
                    let v = ((widen(h) + widen(h + 1)) / 2.0) as f32;
                    let b = (if sign == 0 { v } else { -v }).to_bits();
                    assert_eq!(g.convert(b, false), h | sign);
                    assert_eq!(
                        g.convert(b, true),
                        (if h & 1 == 0 { h } else { h + 1 }) | sign
                    );
                }
            }
        }
    }
    #[test]
    fn corpus_covers_required_signed_boundaries() {
        let c = corpus(0);
        assert_eq!(c.len(), 256);
        assert!(c.iter().all(|b| f32::from_bits(*b).is_finite()));
        for b in [
            0x33000001, 0xb3000001, 0x33c00000, 0xb3c00000, 0x47800000, 0xc7800000, 0x7f7fffff,
            0xff7fffff,
        ] {
            assert!(c.contains(&b));
            assert!(stored_input(&c, 2).contains(&b));
        }
        assert_eq!(&c[..4], &[0x3e800000, 0xbf000000, 0x3f800000, 0x40000000]);
        assert_eq!(corpus(1).len(), 32);
    }
    #[test]
    fn hypotheses_remain_distinct() {
        let g = Grid::new();
        let o = simulated(&g, 2, 1, 0);
        let s = stored_input(&o.echo, 2);
        assert!(profile(&g, &s, &o.pixels, true).mismatch > 0);
        assert_eq!(profile(&g, &s, &o.pixels, false).mismatch, 0);
        assert!(check(&g, &o, 2, 1, 0).pass);
        let mut r = o.clone();
        r.pixels = s.iter().map(|&b| g.convert(b, true)).collect();
        assert!(check(&g, &r, 2, 1, 0).pass);
        assert!(profile(&g, &s, &r.pixels, false).mismatch > 0);
    }
    #[test]
    fn matrix_and_negative_specificity() {
        let g = Grid::new();
        for group in 0..2 {
            for ch in [2, 4] {
                for route in 0..4 {
                    let p = simulated(&g, ch, route, group);
                    assert!(check(&g, &p, ch, route, group).pass);
                    let mut bad = p.clone();
                    bad.pixels[0] = 0xb400;
                    assert!(!check(&g, &bad, ch, route, group).pass);
                    assert!(exact_negative(&p, &bad, ch, group));
                    assert!(!exact_negative(&p, &p, ch, group));
                    for i in 1..bad.pixels.len() {
                        let mut b = bad.clone();
                        b.pixels[i] ^= 1;
                        assert!(!exact_negative(&p, &b, ch, group));
                    }
                    for i in 0..bad.echo.len() {
                        let mut b = bad.clone();
                        b.echo[i] ^= 1;
                        assert!(!exact_negative(&p, &b, ch, group));
                        let mut b = bad.clone();
                        b.casts[i] ^= 1;
                        assert!(!exact_negative(&p, &b, ch, group));
                    }
                }
            }
        }
    }
    #[test]
    fn echo_cast_and_export_fail_independently() {
        let g = Grid::new();
        let p = simulated(&g, 4, 2, 0);
        let mut q = p.clone();
        q.echo[5] ^= 1;
        assert!(!check(&g, &q, 4, 2, 0).pass);
        let mut q = p.clone();
        q.casts[5] ^= 1;
        assert!(!check(&g, &q, 4, 2, 0).pass);
        let mut q = p.clone();
        q.pixels[5] ^= 1;
        assert!(!check(&g, &q, 4, 2, 0).pass);
        let mut q = p.clone();
        q.pixels.pop();
        assert!(!check(&g, &q, 4, 2, 0).pass);
    }
    #[test]
    fn nan_profiles_are_separate() {
        let g = Grid::new();
        let p = profile(&g, &[0x7fc00001, 0xffc12345], &[0x7e00, 0], true);
        assert_eq!(p.non_nan, 0);
        assert_eq!(p.nan_inputs, 2);
        assert_eq!(p.nan_outputs, 1);
        assert_eq!(p.nan_zeros, 1);
    }
    #[test]
    fn abi_and_unavailable_evidence() {
        assert_eq!(std::mem::size_of::<Evidence>(), 48);
        assert!(!Evidence::default().valid(2, 1, 0, 0));
    }
    #[test]
    fn sha_anchor() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
