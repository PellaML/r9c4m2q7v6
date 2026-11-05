use std::{collections::BTreeMap, env, fs, io, path::Path};
const W: usize = 16;
const H: usize = 16;
const N: usize = W * H;
const WORDS: usize = 32;
const UNTOUCHED: u32 = 0xa5a5a5a5;
const SEEDS: [&str; 3] = ["raw-gradient", "finite-clear", "render-gradient"];
const MODES: [&str; 5] = [
    "fetch-only",
    "fetch-independent",
    "fetch-half-dfd",
    "fetch-float-dfd",
    "analytic-no-fetch",
];
const COVER: [&str; 4] = [
    "full",
    "interior-triangle",
    "odd-scissor",
    "triangle-and-scissor",
];
fn err(s: impl Into<String>) -> io::Error {
    io::Error::other(s.into())
}
fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if c < ' ' => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
fn obj(v: &[(&str, String)]) -> String {
    format!(
        "{{{}}}",
        v.iter()
            .map(|(k, v)| format!("{}:{v}", quote(k)))
            .collect::<Vec<_>>()
            .join(",")
    )
}
fn array(v: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", v.into_iter().collect::<Vec<_>>().join(","))
}
fn json(p: impl AsRef<Path>, s: &str) -> io::Result<()> {
    fs::write(p, format!("{s}\n"))
}
fn new_dir(p: &Path) -> io::Result<()> {
    if p.exists() || p.symlink_metadata().is_ok() {
        return Err(err("output must be new"));
    }
    fs::create_dir_all(p)
}
fn bytes16(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn bytes32(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn read16(p: impl AsRef<Path>, n: usize) -> io::Result<Vec<u16>> {
    let b = fs::read(p)?;
    if b.len() != 2 * n {
        return Err(err("wrong u16 length"));
    }
    Ok(b.chunks_exact(2)
        .map(|v| u16::from_le_bytes(v.try_into().unwrap()))
        .collect())
}
fn read32(p: impl AsRef<Path>, n: usize) -> io::Result<Vec<u32>> {
    let b = fs::read(p)?;
    if b.len() != 4 * n {
        return Err(err("wrong u32 length"));
    }
    Ok(b.chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect())
}
fn half(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let e = (b >> 23) & 255;
    let f = b & 0x7fffff;
    if e == 255 {
        return sign | if f == 0 { 0x7c00 } else { 0x7e00 };
    }
    if e > 142 {
        return sign | 0x7c00;
    }
    if e < 102 {
        return sign;
    }
    let (q, rem, mid) = if e < 113 {
        let s = 126 - e;
        let m = 0x800000 | f;
        (m >> s, m & ((1u32 << s) - 1), 1u32 << (s - 1))
    } else {
        (((e - 112) << 10) | (f >> 13), f & 8191, 4096)
    };
    sign | (q + u32::from(rem > mid || (rem == mid && q & 1 != 0))) as u16
}
fn widen(h: u16) -> f32 {
    let s = ((h & 0x8000) as u32) << 16;
    let e = ((h >> 10) & 31) as u32;
    let f = (h & 1023) as u32;
    f32::from_bits(match (e, f) {
        (0, 0) => s,
        (0, _) => {
            let k = 31 - f.leading_zeros();
            s | ((k + 103) << 23) | ((f << (23 - k)) & 0x7fffff)
        }
        (31, _) => s | 0x7f800000 | (f << 13),
        _ => s | ((e + 112) << 23) | (f << 13),
    })
}
fn pack(h: [u16; 2]) -> u32 {
    h[0] as u32 | ((h[1] as u32) << 16)
}
fn seed_value(seed: u32, x: usize, y: usize) -> [f32; 2] {
    if seed == 1 {
        [7., -5.]
    } else {
        [
            8. + 2. * x as f32 + 4. * y as f32,
            32. + 3. * x as f32 - 2. * y as f32,
        ]
    }
}
fn gradient(seed: u32) -> [f32; 4] {
    if seed == 1 {
        [0.; 4]
    } else {
        [2., 3., 4., -2.]
    }
}
fn sentinel() -> [u16; 4] {
    [-101., -102., -103., -104.].map(half)
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Config {
    abi: u32,
    seed: u32,
    mode: u32,
    coverage: u32,
    profile: u32,
    repeat: u32,
    variant: u32,
    pixels: u32,
}
impl Config {
    fn make(seed: u32, mode: u32, coverage: u32, profile: u32, repeat: u32, variant: u32) -> Self {
        Self {
            abi: 1,
            seed,
            mode,
            coverage,
            profile,
            repeat,
            variant,
            pixels: N as u32,
        }
    }
    fn name(self) -> String {
        format!(
            "s{}-m{}-c{}-p{}-r{}-v{}",
            self.seed, self.mode, self.coverage, self.profile, self.repeat, self.variant
        )
    }
    fn scissor(self) -> [u32; 4] {
        match self.coverage {
            2 => [3, 3, 8, 8],
            3 => [5, 3, 6, 6],
            _ => [0, 0, 16, 16],
        }
    }
    fn covered(self, x: usize, y: usize) -> bool {
        let s = self.scissor();
        let rect = x >= s[0] as usize
            && y >= s[1] as usize
            && x < (s[0] + s[2]) as usize
            && y < (s[1] + s[3]) as usize;
        rect && ((self.coverage != 1 && self.coverage != 3) || (x >= 3 && y >= 3 && x + y <= 13))
    }
    fn quad_mask(self, x: usize, y: usize) -> u32 {
        let (qx, qy) = (x & !1, y & !1);
        let mut m = 0;
        for lane in 0..4 {
            if self.covered(qx + lane % 2, qy + lane / 2) {
                m |= 1 << lane;
            }
        }
        m
    }
    fn vertices(self) -> [f32; 12] {
        let second = if self.coverage == 1 || self.coverage == 3 {
            [3.25, 3.25, 11.25, 3.25, 3.25, 11.25]
        } else {
            [0., 0., 32., 0., 0., 32.]
        };
        let mut v = [0.; 12];
        v[..6].copy_from_slice(&[0., 0., 32., 0., 0., 32.]);
        v[6..].copy_from_slice(&second);
        v
    }
    fn dependent(self) -> bool {
        self.mode == 2 || self.mode == 3
    }
}
fn configs() -> Vec<Config> {
    let mut out = Vec::new();
    for profile in 0..2 {
        for seed in 0..3 {
            for mode in 0..5 {
                for coverage in 0..4 {
                    for repeat in 0..2 {
                        out.push(Config::make(seed, mode, coverage, profile, repeat, 0));
                    }
                }
            }
        }
    }
    out
}
fn controls() -> Vec<Config> {
    vec![
        Config::make(0, 0, 0, 0, 0, 1),
        Config::make(0, 1, 0, 0, 0, 2),
        Config::make(0, 0, 2, 0, 0, 3),
    ]
}
fn expected_seed(seed: u32) -> (Vec<u16>, Vec<u16>) {
    let mut a = Vec::new();
    let mut b = Vec::new();
    for y in 0..H {
        for x in 0..W {
            a.extend(sentinel());
            b.extend(seed_value(seed, x, y).map(half));
        }
    }
    (a, b)
}
#[cfg(quad_native)]
fn initial(seed: u32) -> (Vec<u16>, Vec<u16>) {
    if seed == 0 {
        expected_seed(seed)
    } else {
        let a = [-1., -2., -3., -4.].map(half);
        let b = [-11., 13.].map(half);
        (
            (0..N).flat_map(|_| a).collect(),
            (0..N).flat_map(|_| b).collect(),
        )
    }
}
#[repr(C)]
#[derive(Clone, Default)]
struct Evidence {
    abi: u32,
    status: u32,
    completed: u32,
    no_error: u32,
    copied: u32,
    guards: u32,
    seed: u32,
    mode: u32,
    coverage: u32,
    profile: u32,
    repeat: u32,
    variant: u32,
    width: u32,
    height: u32,
    device: u64,
}
impl Evidence {
    #[cfg(any(quad_native, test))]
    fn bytes(&self) -> Vec<u8> {
        let mut b = bytes32(&[
            self.abi,
            self.status,
            self.completed,
            self.no_error,
            self.copied,
            self.guards,
            self.seed,
            self.mode,
            self.coverage,
            self.profile,
            self.repeat,
            self.variant,
            self.width,
            self.height,
        ]);
        b.extend(self.device.to_le_bytes());
        b
    }
    fn read(p: &Path) -> io::Result<Self> {
        let b = fs::read(p)?;
        if b.len() != 64 {
            return Err(err("wrong evidence length"));
        }
        let w: Vec<_> = b[..56]
            .chunks_exact(4)
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        Ok(Self {
            abi: w[0],
            status: w[1],
            completed: w[2],
            no_error: w[3],
            copied: w[4],
            guards: w[5],
            seed: w[6],
            mode: w[7],
            coverage: w[8],
            profile: w[9],
            repeat: w[10],
            variant: w[11],
            width: w[12],
            height: w[13],
            device: u64::from_le_bytes(b[56..].try_into().unwrap()),
        })
    }
}
#[derive(Clone)]
struct Record {
    e: Evidence,
    code: i32,
    metadata: String,
    before0: Vec<u16>,
    before1: Vec<u16>,
    after0: Vec<u16>,
    after1: Vec<u16>,
    trace: Vec<u32>,
}
impl Record {
    #[cfg(any(quad_native, test))]
    fn empty() -> Self {
        Self {
            e: Evidence::default(),
            code: -1,
            metadata: String::new(),
            before0: vec![0xa5a5; N * 4],
            before1: vec![0xa5a5; N * 2],
            after0: vec![0xa5a5; N * 4],
            after1: vec![0xa5a5; N * 2],
            trace: vec![UNTOUCHED; N * WORDS],
        }
    }
    #[cfg(quad_native)]
    fn save(&self, p: &Path) -> io::Result<()> {
        new_dir(p)?;
        json(p.join("native.json"), &self.metadata)?;
        fs::write(p.join("evidence.bin"), self.e.bytes())?;
        fs::write(p.join("ffi-exit.txt"), format!("{}\n", self.code))?;
        if self.code != 0 || self.e.copied != 1 {
            return Ok(());
        }
        for (n, w) in [
            ("before0", &self.before0),
            ("before1", &self.before1),
            ("after0", &self.after0),
            ("after1", &self.after1),
        ] {
            fs::write(p.join(format!("{n}.u16.bin")), bytes16(w))?;
        }
        fs::write(p.join("trace.u32.bin"), bytes32(&self.trace))
    }
    fn load(p: &Path) -> io::Result<Self> {
        Ok(Self {
            e: Evidence::read(&p.join("evidence.bin"))?,
            code: fs::read_to_string(p.join("ffi-exit.txt"))?
                .trim()
                .parse()
                .map_err(|_| err("bad exit"))?,
            metadata: fs::read_to_string(p.join("native.json"))?,
            before0: read16(p.join("before0.u16.bin"), N * 4)?,
            before1: read16(p.join("before1.u16.bin"), N * 2)?,
            after0: read16(p.join("after0.u16.bin"), N * 4)?,
            after1: read16(p.join("after1.u16.bin"), N * 2)?,
            trace: read32(p.join("trace.u32.bin"), N * WORDS)?,
        })
    }
}
#[derive(Default)]
struct Checks(BTreeMap<&'static str, usize>);
impl Checks {
    fn add(&mut self, k: &'static str, n: usize) {
        if n > 0 {
            *self.0.entry(k).or_default() += n;
        }
    }
    fn ok(&self) -> bool {
        self.0.is_empty()
    }
    fn has(&self, k: &str) -> bool {
        self.0.contains_key(k)
    }
    fn json(&self) -> String {
        obj(&self
            .0
            .iter()
            .map(|(k, v)| (*k, v.to_string()))
            .collect::<Vec<_>>())
    }
}
fn expected_color(c: Config, x: usize, y: usize) -> [u16; 4] {
    let s = seed_value(c.seed, x, y);
    match c.mode {
        0 => [s[0], s[1], 19., 23.],
        1 => [s[0] + 1., s[1] + 1., 3., 5.],
        _ => gradient(c.seed),
    }
    .map(half)
}
fn check(c: Config, r: &Record) -> Checks {
    let mut o = Checks::default();
    let e = &r.e;
    o.add(
        "native_completion",
        usize::from(
            r.code != 0
                || e.abi != 1
                || e.status != 4
                || e.completed != 1
                || e.no_error != 1
                || e.copied != 1
                || e.guards != 1
                || e.device == 0,
        ),
    );
    o.add(
        "native_identity",
        usize::from(
            e.seed != c.seed
                || e.mode != c.mode
                || e.coverage != c.coverage
                || e.profile != c.profile
                || e.repeat != c.repeat
                || e.variant != c.variant
                || e.width != W as u32
                || e.height != H as u32,
        ),
    );
    o.add("metadata", usize::from(r.metadata.trim().is_empty()));
    o.add(
        "length",
        usize::from(
            r.before0.len() != N * 4
                || r.after0.len() != N * 4
                || r.before1.len() != N * 2
                || r.after1.len() != N * 2
                || r.trace.len() != N * WORDS,
        ),
    );
    if o.has("length") {
        return o;
    }
    let (b0, b1) = expected_seed(c.seed);
    o.add(
        "seed0",
        r.before0.iter().zip(&b0).filter(|(a, b)| a != b).count(),
    );
    o.add(
        "seed1",
        r.before1.iter().zip(&b1).filter(|(a, b)| a != b).count(),
    );
    o.add(
        "attachment_not_preserved",
        r.after1.iter().zip(&b1).filter(|(a, b)| a != b).count(),
    );
    for i in 0..N {
        let (x, y) = (i % W, i / W);
        let t = &r.trace[i * WORDS..(i + 1) * WORDS];
        if !c.covered(x, y) {
            o.add(
                "inactive_trace_write",
                usize::from(t.iter().any(|&v| v != UNTOUCHED)),
            );
            o.add(
                "inactive_color_write",
                r.after0[i * 4..i * 4 + 4]
                    .iter()
                    .zip(sentinel())
                    .filter(|(a, b)| **a != *b)
                    .count(),
            );
            continue;
        }
        o.add(
            "active_stamp",
            usize::from(t[0] != (0x13570000 | (i as u32 + 1))),
        );
        o.add(
            "trace_contract",
            usize::from(
                t[1] != c.mode
                    || t[7] != c.seed
                    || t[25] != c.variant
                    || t[26..].iter().any(|&v| v != 0),
            ),
        );
        let px = x as f32 + 0.5;
        let py = y as f32 + 0.5;
        let uv = [3. * px + 5. * py + 1., 2. * px - 4. * py + 7.];
        o.add(
            "position",
            usize::from(t[2] != px.to_bits() || t[3] != py.to_bits()),
        );
        o.add(
            "varying",
            usize::from(t[4] != uv[0].to_bits() || t[5] != uv[1].to_bits()),
        );
        let prior = pack(seed_value(c.seed, x, y).map(half));
        o.add(
            "active_operand",
            usize::from(t[6] != prior || t[24] != prior),
        );
        let pd: [f32; 4] = if c.mode == 0 {
            [0.; 4]
        } else {
            [1., 0., 0., 1.]
        };
        let vd: [f32; 4] = if c.mode == 0 {
            [0.; 4]
        } else {
            [3., 2., 5., -4.]
        };
        o.add(
            "position_derivative",
            t[8..12]
                .iter()
                .zip(pd)
                .filter(|(a, b)| **a != b.to_bits())
                .count(),
        );
        o.add(
            "varying_derivative",
            t[12..16]
                .iter()
                .zip(vd)
                .filter(|(a, b)| **a != b.to_bits())
                .count(),
        );
        let defined = !c.dependent() || c.quad_mask(x, y) == 15;
        if defined {
            let want = expected_color(c, x, y);
            o.add(
                "defined_color",
                r.after0[i * 4..i * 4 + 4]
                    .iter()
                    .zip(want)
                    .filter(|(a, b)| **a != *b)
                    .count(),
            );
            o.add(
                "defined_output_trace",
                usize::from(t[22] != pack([want[0], want[1]]) || t[23] != pack([want[2], want[3]])),
            );
            let d = if c.mode < 2 {
                [0.; 4]
            } else {
                gradient(c.seed)
            };
            o.add(
                "defined_operand_derivative",
                t[16..20]
                    .iter()
                    .zip(d)
                    .filter(|(a, b)| **a != b.to_bits())
                    .count(),
            );
        }
        if c.mode == 0 || c.mode == 1 || c.mode == 3 {
            o.add(
                "unused_half_derivative_fields",
                usize::from(t[20] != 0 || t[21] != 0),
            );
        }
    }
    o
}
fn zero_model(c: Config, x: usize, y: usize, row: usize, col: usize) -> [u32; 4] {
    let (qx, qy) = (x & !1, y & !1);
    let value = |xx, yy| {
        if c.covered(xx, yy) {
            seed_value(c.seed, xx, yy)
        } else {
            [0.; 2]
        }
    };
    let a = value(qx, qy + row);
    let b = value(qx + 1, qy + row);
    let d = value(qx + col, qy);
    let e = value(qx + col, qy + 1);
    [b[0] - a[0], b[1] - a[1], e[0] - d[0], e[1] - d[1]].map(|v| {
        if c.mode == 2 {
            widen(half(v)).to_bits()
        } else {
            v.to_bits()
        }
    })
}
#[derive(Default)]
struct Stats {
    active: usize,
    full: usize,
    helper: usize,
    stored: usize,
    zero_fine: usize,
    zero_any: usize,
    nonfinite: usize,
    other: usize,
}
fn stats(c: Config, r: &Record, rows: &mut String) -> Stats {
    let mut s = Stats::default();
    for i in 0..N {
        let (x, y) = (i % W, i / W);
        if !c.covered(x, y) {
            continue;
        }
        s.active += 1;
        if !c.dependent() {
            continue;
        }
        let mask = c.quad_mask(x, y);
        let t = &r.trace[i * WORDS..(i + 1) * WORDS];
        let actual: &[u32] = &t[16..20];
        if mask == 15 {
            s.full += 1;
            continue;
        }
        s.helper += 1;
        let stored = gradient(c.seed).map(f32::to_bits);
        let z = zero_model(c, x, y, y & 1, x & 1);
        let stored_ok = actual == stored;
        let zero_ok = actual == z;
        let any = (0..2).any(|row| (0..2).any(|col| actual == zero_model(c, x, y, row, col)));
        let nonfinite = actual.iter().any(|&b| !f32::from_bits(b).is_finite());
        s.stored += usize::from(stored_ok);
        s.zero_fine += usize::from(zero_ok);
        s.zero_any += usize::from(any);
        s.nonfinite += usize::from(nonfinite);
        s.other += usize::from(!stored_ok && !any);
        rows.push_str(&format!("{},{x},{y},{mask:01x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{:08x},{stored_ok},{zero_ok},{any},{nonfinite}\n",c.name(),t[6],actual[0],actual[1],actual[2],actual[3],z[0],z[1],z[2],z[3],t[22]));
    }
    s
}
fn plan(root: &Path) -> io::Result<()> {
    new_dir(root)?;
    let mut coverage_csv = String::from("coverage,x,y,active,quad_mask\n");
    for seed in 0..3 {
        let (a, b) = expected_seed(seed);
        fs::write(
            root.join(format!("seed{seed}-expected0.u16.bin")),
            bytes16(&a),
        )?;
        fs::write(
            root.join(format!("seed{seed}-expected1.u16.bin")),
            bytes16(&b),
        )?;
    }
    for cov in 0..4 {
        let c = Config::make(0, 0, cov, 0, 0, 0);
        let v: Vec<u32> = c.vertices().iter().map(|v| v.to_bits()).collect();
        fs::write(
            root.join(format!("coverage{cov}-vertices.u32.bin")),
            bytes32(&v),
        )?;
        for y in 0..H {
            for x in 0..W {
                coverage_csv.push_str(&format!(
                    "{cov},{x},{y},{},{:x}\n",
                    c.covered(x, y),
                    c.quad_mask(x, y)
                ));
            }
        }
    }
    fs::write(root.join("coverage.csv"), coverage_csv)?;
    json(root.join("contract.json"),&obj(&[("schema","1".into()),("kind",quote("portable_synthetic_inputs_not_native_results")),("width",W.to_string()),("height",H.to_string()),
        ("normal_cases",configs().len().to_string()),("native_controls",controls().len().to_string()),("trace_words_per_pixel",WORDS.to_string()),
        ("seed_variants",array(SEEDS.iter().map(|s|quote(s)))),("shader_modes",array(MODES.iter().map(|s|quote(s)))),("coverage_modes",array(COVER.iter().map(|s|quote(s)))),
        ("compiler_profiles",array([quote("runtime-default"),quote("fast-math-disabled")])),("repetitions","2".into()),
        ("attachment_layout",quote("RGBA16Float color0 diagnostic; RG16Float color1 half2 framebuffer input and unchanged half2 output; no blending")),
        ("seed_formula",quote("R=8+2*x+4*y; G=32+3*x-2*y; clear reference=(7,-5); finite exact half values")),
        ("varying_formula",quote("u=3*position.x+5*position.y+1; v=2*position.x-4*position.y+7; w=1")),
        ("helper_contract",quote("All 2x2 members inside 16x16 backing. Stored-neighbor and zero-helper derivative models are hypotheses only. No quad shuffle of inactive threads. No image-edge promotion.")),
        ("trace_layout",quote("0 stamp;1 mode;2:4 position f32;4:6 varying f32;6 packed half2 operand;7 seed;8:12 position dfd;12:16 varying dfd;16:20 operand dfd f32;20:22 packed half dfd;22:24 packed half4 diagnostic;24 packed half2 output;25 fault;26:32 zero"))]))
}
struct Report {
    text: String,
    table: String,
    rows: String,
    passed: bool,
}
fn report(root: &Path) -> io::Result<Report> {
    let cases = configs();
    let mut records = Vec::new();
    let mut checks = Vec::new();
    let mut rows=String::from("case,x,y,quad_mask,operand_half2,dx_r,dx_g,dy_r,dy_g,zero_dx_r,zero_dx_g,zero_dy_r,zero_dy_g,output0_rg_half,stored_model,zero_fine_model,zero_any_pair_model,nonfinite_derivative\n");
    let mut table=String::from("case,seed,mode,coverage,profile,repeat,active,full_quad_active,helper_cohort_active,stored_model_matches,zero_fine_matches,zero_any_pair_matches,nonfinite,neither_model,check_passed\n");
    let mut passed = true;
    let mut helpers = 0;
    let mut stored = 0;
    let mut zero = 0;
    let mut any = 0;
    let mut nonfinite = 0;
    let mut neither = 0;
    for &c in &cases {
        let r = Record::load(&root.join("cases").join(c.name()))?;
        let ck = check(c, &r);
        passed &= ck.ok();
        let s = stats(c, &r, &mut rows);
        helpers += s.helper;
        stored += s.stored;
        zero += s.zero_fine;
        any += s.zero_any;
        nonfinite += s.nonfinite;
        neither += s.other;
        table.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            c.name(),
            SEEDS[c.seed as usize],
            MODES[c.mode as usize],
            COVER[c.coverage as usize],
            c.profile,
            c.repeat,
            s.active,
            s.full,
            s.helper,
            s.stored,
            s.zero_fine,
            s.zero_any,
            s.nonfinite,
            s.other,
            ck.ok()
        ));
        checks.push(obj(&[
            ("case", quote(&c.name())),
            ("violations", ck.json()),
        ]));
        records.push(r);
    }
    let mut repeat_different = 0;
    let mut repeat_defined = 0;
    let mut repeat_helpers = 0;
    for k in (0..cases.len()).step_by(2) {
        let c = cases[k];
        let a = &records[k];
        let b = &records[k + 1];
        let different = a.before0 != b.before0
            || a.before1 != b.before1
            || a.after0 != b.after0
            || a.after1 != b.after1
            || a.trace != b.trace;
        repeat_different += usize::from(different);
        for i in 0..N {
            if a.after0[i * 4..i * 4 + 4] != b.after0[i * 4..i * 4 + 4]
                || a.after1[i * 2..i * 2 + 2] != b.after1[i * 2..i * 2 + 2]
                || a.trace[i * WORDS..i * WORDS + WORDS] != b.trace[i * WORDS..i * WORDS + WORDS]
            {
                if c.dependent() && c.covered(i % W, i / W) && c.quad_mask(i % W, i / W) != 15 {
                    repeat_helpers += 1;
                } else {
                    repeat_defined += 1;
                }
            }
        }
    }
    let mut negatives = Vec::new();
    for c in controls() {
        let r = Record::load(&root.join("controls").join(c.name()))?;
        let ck = check(c, &r);
        let caught = match c.variant {
            1 => ck.has("defined_color"),
            2 => ck.has("varying_derivative") && ck.has("defined_color"),
            3 => ck.has("inactive_trace_write") && ck.has("active_stamp"),
            _ => false,
        };
        passed &= caught;
        negatives.push(obj(&[
            ("case", quote(&c.name())),
            ("specific_fault_detected", caught.to_string()),
            ("violations", ck.json()),
        ]));
    }
    let text=obj(&[("schema","1".into()),("backend",quote("native_metal_completed_readbacks")),("integrity_and_defined_math_passed",passed.to_string()),
        ("normal_cases",cases.len().to_string()),("helper_cohort_active_observations",helpers.to_string()),("stored_neighbor_model_matches",stored.to_string()),
        ("zero_helper_fine_model_matches",zero.to_string()),("zero_helper_any_pair_model_matches",any.to_string()),("nonfinite_helper_derivative_observations",nonfinite.to_string()),
        ("neither_helper_model",neither.to_string()),("repeat_pairs",(cases.len()/2).to_string()),("repeat_pairs_different",repeat_different.to_string()),
        ("repeat_different_defined_pixels",repeat_defined.to_string()),("repeat_different_helper_pixels",repeat_helpers.to_string()),
        ("hypotheses_are_oracles","false".into()),("helper_fetch_contract_established","false".into()),
        ("qualification",quote("Active fetched values and known derivatives checked separately from attachment-dependent helper observations. No helper value invented; no API-creation-only inference.")),
        ("normal_checks",array(checks)),("negative_controls",array(negatives))]);
    Ok(Report {
        text,
        table,
        rows,
        passed,
    })
}
#[cfg(quad_native)]
extern "C" {
    fn quad_fetch_native(
        r: *const Config,
        source: *const std::ffi::c_char,
        initial0: *const u16,
        initial1: *const u16,
        vertices: *const f32,
        scissor: *const u32,
        before0: *mut u16,
        before1: *mut u16,
        after0: *mut u16,
        after1: *mut u16,
        trace: *mut u32,
        e: *mut Evidence,
        json: *mut std::ffi::c_char,
        cap: usize,
    ) -> i32;
}
#[cfg(quad_native)]
fn native(c: Config) -> io::Result<Record> {
    let source = std::ffi::CString::new(include_str!("probe.metal")).unwrap();
    let (a, b) = initial(c.seed);
    let v = c.vertices();
    let sc = c.scissor();
    let mut r = Record::empty();
    let mut j = vec![0u8; 65536];
    r.code = unsafe {
        quad_fetch_native(
            &c,
            source.as_ptr(),
            a.as_ptr(),
            b.as_ptr(),
            v.as_ptr(),
            sc.as_ptr(),
            r.before0.as_mut_ptr(),
            r.before1.as_mut_ptr(),
            r.after0.as_mut_ptr(),
            r.after1.as_mut_ptr(),
            r.trace.as_mut_ptr(),
            &mut r.e,
            j.as_mut_ptr().cast(),
            j.len(),
        )
    };
    let end = j
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| err("unterminated native metadata"))?;
    r.metadata = String::from_utf8(j[..end].to_vec()).map_err(|_| err("invalid native UTF8"))?;
    if r.metadata.is_empty() {
        r.metadata = obj(&[
            ("status", quote("native_metadata_missing")),
            ("ffi_exit", r.code.to_string()),
        ]);
    }
    Ok(r)
}
#[cfg(quad_native)]
fn observe(root: &Path) -> io::Result<()> {
    new_dir(root)?;
    plan(&root.join("inputs"))?;
    for (sub, cases) in [("cases", configs()), ("controls", controls())] {
        for c in cases {
            let r = native(c)?;
            r.save(&root.join(sub).join(c.name()))?;
            println!("{} native_exit={} copied={}", c.name(), r.code, r.e.copied);
            if r.code != 0 || r.e.copied != 1 {
                return Err(err(format!(
                    "native failure in {}; exact diagnostic retained; no fallback",
                    c.name()
                )));
            }
        }
    }
    let r = report(root)?;
    json(root.join("summary.json"), &r.text)?;
    fs::write(root.join("case-summary.csv"), r.table)?;
    fs::write(root.join("helper-observations.csv"), r.rows)?;
    if !r.passed {
        return Err(err(
            "defined-math or transport/control checks failed; observations retained",
        ));
    }
    Ok(())
}
#[cfg(not(quad_native))]
fn observe(_: &Path) -> io::Result<()> {
    Err(err("native macOS binary required; no CPU-as-GPU fallback"))
}
fn run() -> io::Result<()> {
    let a: Vec<_> = env::args().collect();
    if a.len() != 3 {
        return Err(err("usage: probe plan|observe|check DIRECTORY"));
    }
    let p = Path::new(&a[2]);
    match a[1].as_str() {
        "plan" => plan(p),
        "observe" => observe(p),
        "check" => {
            let r = report(p)?;
            eprintln!("regenerated_csv_bytes={},{}", r.table.len(), r.rows.len());
            println!("{}", r.text);
            if r.passed {
                Ok(())
            } else {
                Err(err("checks failed"))
            }
        }
        _ => Err(err("unknown command")),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn synthetic(c: Config) -> Record {
        let mut r = Record::empty();
        let (a, b) = expected_seed(c.seed);
        r.before0 = a.clone();
        r.before1 = b.clone();
        r.after0 = a;
        r.after1 = b;
        r.code = 0;
        r.metadata = "TEST ONLY synthetic checker data; NOT GPU evidence".into();
        r.e = Evidence {
            abi: 1,
            status: 4,
            completed: 1,
            no_error: 1,
            copied: 1,
            guards: 1,
            seed: c.seed,
            mode: c.mode,
            coverage: c.coverage,
            profile: c.profile,
            repeat: c.repeat,
            variant: c.variant,
            width: W as u32,
            height: H as u32,
            device: 1,
        };
        for i in 0..N {
            let (x, y) = (i % W, i / W);
            if !c.covered(x, y) {
                continue;
            }
            let t = &mut r.trace[i * WORDS..i * WORDS + WORDS];
            t.fill(0);
            t[0] = 0x13570000 | (i as u32 + 1);
            t[1] = c.mode;
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            t[2] = px.to_bits();
            t[3] = py.to_bits();
            t[4] = (3. * px + 5. * py + 1.).to_bits();
            t[5] = (2. * px - 4. * py + 7.).to_bits();
            t[6] = pack(seed_value(c.seed, x, y).map(half));
            t[7] = c.seed;
            if c.mode != 0 {
                t[8..12].copy_from_slice(&[1., 0., 0., 1.].map(f32::to_bits));
                t[12..16].copy_from_slice(&[3., 2., 5., -4.].map(f32::to_bits));
            }
            if c.mode >= 2 {
                t[16..20].copy_from_slice(&gradient(c.seed).map(f32::to_bits));
            }
            let color = expected_color(c, x, y);
            r.after0[i * 4..i * 4 + 4].copy_from_slice(&color);
            t[22] = pack([color[0], color[1]]);
            t[23] = pack([color[2], color[3]]);
            t[24] = t[6];
            t[25] = c.variant;
        }
        r
    }
    fn c() -> Config {
        Config::make(0, 1, 2, 0, 0, 0)
    }
    #[test]
    fn abi() {
        assert_eq!(std::mem::size_of::<Config>(), 32);
        assert_eq!(std::mem::size_of::<Evidence>(), 64);
        assert_eq!(Evidence::default().bytes().len(), 64);
    }
    #[test]
    fn half_exact_roundtrip() {
        for h in 0..=u16::MAX {
            if h & 0x7c00 != 0x7c00 {
                assert_eq!(half(widen(h)), h);
            }
        }
    }
    #[test]
    fn matrix_and_coverage() {
        assert_eq!(configs().len(), 240);
        let mut counts = Vec::new();
        for cov in 0..4 {
            let c = Config::make(0, 0, cov, 0, 0, 0);
            counts.push((0..N).filter(|&i| c.covered(i % W, i / W)).count());
            for i in 0..N {
                assert!((i % W & !1) + 1 < W && (i / W & !1) + 1 < H);
            }
        }
        assert_eq!(counts, vec![256, 36, 64, 21]);
    }
    #[test]
    fn raw_and_clear_inputs_are_finite_exact() {
        for seed in 0..3 {
            let (a, b) = expected_seed(seed);
            assert!(a.iter().chain(&b).all(|&h| widen(h).is_finite()));
            for y in 0..H {
                for x in 0..W {
                    for v in seed_value(seed, x, y) {
                        assert_eq!(widen(half(v)), v);
                    }
                }
            }
        }
    }
    #[test]
    fn derivative_models_discriminate() {
        let c = Config::make(0, 2, 2, 0, 0, 0);
        assert_ne!(c.quad_mask(3, 3), 15);
        assert_ne!(zero_model(c, 3, 3, 1, 1), gradient(0).map(f32::to_bits));
        assert_eq!(zero_model(c, 4, 4, 0, 0), gradient(0).map(f32::to_bits));
    }
    #[test]
    fn synthetic_defined_positive() {
        for config in configs() {
            assert!(check(config, &synthetic(config)).ok(), "{}", config.name());
        }
    }
    #[test]
    fn negative_completion() {
        let c = c();
        let mut r = synthetic(c);
        r.e.completed = 0;
        assert!(check(c, &r).has("native_completion"));
    }
    #[test]
    fn negative_missing_metadata() {
        let c = c();
        let mut r = synthetic(c);
        r.metadata.clear();
        assert!(check(c, &r).has("metadata"));
    }
    #[test]
    fn negative_truncation() {
        let c = c();
        let mut r = synthetic(c);
        r.trace.pop();
        assert!(check(c, &r).has("length"));
    }
    #[test]
    fn negative_seed_bytes() {
        let c = c();
        let mut r = synthetic(c);
        r.before1[0] ^= 1;
        assert!(check(c, &r).has("seed1"));
    }
    #[test]
    fn negative_changed_attachment() {
        let c = c();
        let mut r = synthetic(c);
        r.after1[100] ^= 1;
        assert!(check(c, &r).has("attachment_not_preserved"));
    }
    #[test]
    fn negative_uncovered_write() {
        let c = c();
        let mut r = synthetic(c);
        r.after0[0] ^= 1;
        assert!(check(c, &r).has("inactive_color_write"));
    }
    #[test]
    fn negative_helper_side_effect() {
        let c = c();
        let mut r = synthetic(c);
        r.trace[0] = 1;
        assert!(check(c, &r).has("inactive_trace_write"));
    }
    #[test]
    fn negative_missing_active() {
        let c = c();
        let mut r = synthetic(c);
        r.trace[(3 * W + 3) * WORDS] = UNTOUCHED;
        assert!(check(c, &r).has("active_stamp"));
    }
    #[test]
    fn negative_position_derivative() {
        let c = c();
        let mut r = synthetic(c);
        r.trace[(3 * W + 3) * WORDS + 8] = 2f32.to_bits();
        assert!(check(c, &r).has("position_derivative"));
    }
    #[test]
    fn negative_varying_derivative() {
        let c = c();
        let mut r = synthetic(c);
        r.trace[(3 * W + 3) * WORDS + 12] = 4f32.to_bits();
        assert!(check(c, &r).has("varying_derivative"));
    }
    #[test]
    fn negative_active_fetch() {
        let c = c();
        let mut r = synthetic(c);
        r.trace[(3 * W + 3) * WORDS + 6] ^= 1;
        assert!(check(c, &r).has("active_operand"));
    }
    #[test]
    fn negative_defined_color() {
        let c = c();
        let mut r = synthetic(c);
        r.after0[(3 * W + 3) * 4] ^= 1;
        assert!(check(c, &r).has("defined_color"));
    }
    #[test]
    fn negative_defined_attachment_derivative() {
        let c = Config::make(0, 2, 0, 0, 0, 0);
        let mut r = synthetic(c);
        r.trace[16] = 0;
        assert!(check(c, &r).has("defined_operand_derivative"));
    }
    #[test]
    fn helper_hypothesis_is_not_an_acceptance_oracle() {
        let c = Config::make(0, 2, 2, 0, 0, 0);
        let mut r = synthetic(c);
        let i = 3 * W + 3;
        r.trace[i * WORDS + 16] = 123f32.to_bits();
        let mut rows = String::new();
        assert!(check(c, &r).ok());
        let s = stats(c, &r, &mut rows);
        assert!(s.other > 0);
        assert!(rows.contains("42f60000"));
    }
}
