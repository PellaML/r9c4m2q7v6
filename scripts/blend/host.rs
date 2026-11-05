use std::{env, fs, io, path::Path};

// Independently authored public experiment. The existing public boundary and
// two-channel probes supplied only harness/API idioms, not an application model.
const ROUTES: [&str; 4] = ["float", "half-words", "widen-words", "cast-half"];
const LAYOUTS: [&str; 4] = ["rg", "rgba", "mrt-first-all", "mrt-first-none"];
const DEST: [u16; 16] = [
    0x0000, 0x8000, 0x7c00, 0xfc00, 0x7e35, 0xfe53, 0x7c01, 0xfc23, 0x3800, 0xb800, 0x3c00, 0xbc00,
    0x0001, 0x8001, 0x7bff, 0xfbff,
];
const HALVES: [u16; 16] = [
    0x3c00, 0x3800, 0x4000, 0x0000, 0x0001, 0x03ff, 0x0400, 0x1000, 0x3400, 0x3c01, 0x3e00, 0x4200,
    0x4900, 0x6400, 0x7bfe, 0x7bff,
];
const LOWER: [u16; 16] = [
    0x0000, 0x0001, 0x03fe, 0x03ff, 0x0400, 0x33ff, 0x37ff, 0x3800, 0x3bff, 0x3c00, 0x3c01, 0x3e00,
    0x3fff, 0x4000, 0x63ff, 0x7bfe,
];
const SHIFTS: [usize; 4] = [0, 7, 13, 23];

fn err(s: impl Into<String>) -> io::Error {
    io::Error::other(s.into())
}
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
fn object(fields: &[(&str, String)]) -> String {
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
fn json(path: impl AsRef<Path>, value: &str) -> io::Result<()> {
    fs::write(path, format!("{value}\n"))
}
fn new_dir(path: &Path) -> io::Result<()> {
    if path.exists() || path.symlink_metadata().is_ok() {
        return Err(err("output must be new"));
    }
    fs::create_dir_all(path)
}
fn u16_bytes(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn u32_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn read16(path: impl AsRef<Path>, count: usize) -> io::Result<Vec<u16>> {
    let b = fs::read(path)?;
    if b.len() != count * 2 {
        return Err(err("wrong u16 file length"));
    }
    Ok(b.chunks_exact(2)
        .map(|x| u16::from_le_bytes(x.try_into().unwrap()))
        .collect())
}
fn read32(path: impl AsRef<Path>, count: usize) -> io::Result<Vec<u32>> {
    let b = fs::read(path)?;
    if b.len() != count * 4 {
        return Err(err("wrong u32 file length"));
    }
    Ok(b.chunks_exact(4)
        .map(|x| u32::from_le_bytes(x.try_into().unwrap()))
        .collect())
}

// Pure integer widening is exact. No native half instruction or project helper.
fn widen(h: u16) -> u32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let e = ((h >> 10) & 31) as u32;
    let f = (h & 1023) as u32;
    match (e, f) {
        (0, 0) => sign,
        (0, _) => {
            let k = 31 - f.leading_zeros();
            sign | ((k + 103) << 23) | ((f << (23 - k)) & 0x7fffff)
        }
        (31, _) => sign | 0x7f800000 | (f << 13),
        _ => sign | ((e + 112) << 23) | (f << 13),
    }
}
struct Lattice(Vec<f64>);
impl Lattice {
    fn new() -> Self {
        Self(
            (0..=0x7bff)
                .map(|h| f32::from_bits(widen(h)) as f64)
                .collect(),
        )
    }
    // These are named hypotheses, NEVER a claimed Metal export oracle.
    fn convert(&self, bits: u32, rne: bool) -> u16 {
        let sign = ((bits >> 16) & 0x8000) as u16;
        let x = (f32::from_bits(bits) as f64).abs();
        if x.is_nan() {
            return sign | 0x7e00;
        }
        if x.is_infinite() {
            return sign | 0x7c00;
        }
        let lo = self.0.partition_point(|&v| v <= x) - 1;
        if !rne || self.0[lo] == x {
            return sign | lo as u16;
        }
        let hi = lo + 1;
        let index = if hi == 0x7c00 {
            if x >= 65520.0 {
                hi
            } else {
                lo
            }
        } else {
            let dl = x - self.0[lo];
            let dh = self.0[hi] - x;
            if dl < dh || (dl == dh && lo % 2 == 0) {
                lo
            } else {
                hi
            }
        };
        sign | index as u16
    }
}
#[derive(Clone)]
struct Value {
    bits: u32,
    raw: u16,
    kind: &'static str,
}
fn pool(route: u32) -> Vec<Value> {
    let mut out = Vec::new();
    for h in HALVES {
        for sign in [0u16, 0x8000] {
            let raw = h | sign;
            out.push(Value {
                bits: widen(raw),
                raw,
                kind: "exact-half",
            });
        }
    }
    if route == 0 || route == 3 {
        for h in LOWER {
            let mid = ((f32::from_bits(widen(h)) as f64 + f32::from_bits(widen(h + 1)) as f64)
                / 2.0) as f32;
            for (bits, kind) in [
                (mid.to_bits() - 1, "mid-below"),
                (mid.to_bits(), "mid-tie"),
                (mid.to_bits() + 1, "mid-above"),
            ] {
                for sign in [0u32, 0x80000000] {
                    out.push(Value {
                        bits: bits | sign,
                        raw: h | ((sign >> 16) as u16),
                        kind,
                    });
                }
            }
        }
        let top = 65520f32.to_bits();
        for bits in [
            top - 1,
            top,
            top + 1,
            65536f32.to_bits(),
            f32::MAX.to_bits(),
        ] {
            for sign in [0u32, 0x80000000] {
                out.push(Value {
                    bits: bits | sign,
                    raw: 0x7bff | ((sign >> 16) as u16),
                    kind: "finite-overflow-boundary",
                });
            }
        }
    }
    out
}
struct Fixture {
    pool: Vec<Value>,
    src: Vec<u32>,
    raw: Vec<u16>,
    dst: Vec<u16>,
}
impl Fixture {
    fn new(route: u32) -> Self {
        let pool = pool(route);
        let mut src = Vec::new();
        let mut raw = Vec::new();
        let mut dst = Vec::new();
        for row in 0..pool.len() {
            for col in 0..DEST.len() {
                for (c, shift) in SHIFTS.iter().enumerate() {
                    let v = &pool[(row + shift) % pool.len()];
                    src.push(v.bits);
                    raw.push(v.raw);
                    dst.push(DEST[(col + 3 * c) % DEST.len()]);
                }
            }
        }
        Self {
            pool,
            src,
            raw,
            dst,
        }
    }
    fn count(&self) -> usize {
        self.src.len() / 4
    }
    fn value(&self, pixel: usize, channel: usize) -> &Value {
        &self.pool[(pixel / 16 + SHIFTS[channel]) % self.pool.len()]
    }
}
#[derive(Clone, Copy)]
struct Config {
    route: u32,
    layout: u32,
    mask: u32,
    enabled: u32,
    variant: u32,
}
impl Config {
    fn name(self) -> String {
        format!(
            "{}-{}-{}-{}-v{}",
            ROUTES[self.route as usize],
            LAYOUTS[self.layout as usize],
            if self.mask == 8 { "red" } else { "all" },
            if self.enabled == 0 { "off" } else { "on" },
            self.variant
        )
    }
    fn channels(self) -> usize {
        if self.layout == 1 {
            4
        } else {
            2
        }
    }
    fn mrt(self) -> bool {
        self.layout >= 2
    }
    fn written(self, c: usize) -> bool {
        c == 0 || self.mask == 15
    }
}
fn configurations() -> Vec<Config> {
    let mut out = Vec::new();
    for route in 0..4 {
        for layout in 0..4 {
            for mask in [8, 15] {
                for enabled in 0..2 {
                    out.push(Config {
                        route,
                        layout,
                        mask,
                        enabled,
                        variant: 0,
                    });
                }
            }
        }
    }
    out
}
fn controls() -> Vec<Config> {
    (1..=3)
        .map(|variant| Config {
            route: 0,
            layout: 0,
            mask: 8,
            enabled: 1,
            variant,
        })
        .collect()
}
#[repr(C)]
#[derive(Clone, Default)]
struct Evidence {
    abi: u32,
    command_status: u32,
    completed: u32,
    no_error: u32,
    copied: u32,
    guards: u32,
    channels: u32,
    layout: u32,
    route: u32,
    enabled: u32,
    mask: u32,
    variant: u32,
    count: u32,
    dst_factor: u32,
    device_id: u64,
}
impl Evidence {
    #[cfg(any(blend_native, test))]
    fn bytes(&self) -> Vec<u8> {
        let mut b = u32_bytes(&[
            self.abi,
            self.command_status,
            self.completed,
            self.no_error,
            self.copied,
            self.guards,
            self.channels,
            self.layout,
            self.route,
            self.enabled,
            self.mask,
            self.variant,
            self.count,
            self.dst_factor,
        ]);
        b.extend_from_slice(&self.device_id.to_le_bytes());
        b
    }
    fn read(path: &Path) -> io::Result<Self> {
        let b = fs::read(path)?;
        if b.len() != 64 {
            return Err(err("wrong evidence ABI length"));
        }
        let w: Vec<_> = b[..56]
            .chunks_exact(4)
            .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
            .collect();
        Ok(Self {
            abi: w[0],
            command_status: w[1],
            completed: w[2],
            no_error: w[3],
            copied: w[4],
            guards: w[5],
            channels: w[6],
            layout: w[7],
            route: w[8],
            enabled: w[9],
            mask: w[10],
            variant: w[11],
            count: w[12],
            dst_factor: w[13],
            device_id: u64::from_le_bytes(b[56..].try_into().unwrap()),
        })
    }
}
#[derive(Clone)]
struct Record {
    evidence: Evidence,
    code: i32,
    metadata: String,
    before: Vec<u16>,
    after: Vec<u16>,
    first_before: Vec<u16>,
    first_after: Vec<u16>,
    seen: Vec<u32>,
    seen_half: Vec<u16>,
    export_float: Vec<u32>,
    export_half: Vec<u16>,
}
impl Record {
    #[cfg(any(blend_native, test))]
    fn empty(c: Config, n: usize) -> Self {
        Self {
            evidence: Evidence::default(),
            code: -1,
            metadata: String::new(),
            before: vec![0xa5a5; n * c.channels()],
            after: vec![0xa5a5; n * c.channels()],
            first_before: vec![0xa5a5; if c.mrt() { n * 4 } else { 0 }],
            first_after: vec![0xa5a5; if c.mrt() { n * 4 } else { 0 }],
            seen: vec![0xa5a5a5a5; n * 4],
            seen_half: vec![0xa5a5; n * 4],
            export_float: vec![0xa5a5a5a5; n * 4],
            export_half: vec![0xa5a5; n * 4],
        }
    }
    #[cfg(blend_native)]
    fn save(&self, dir: &Path) -> io::Result<()> {
        new_dir(dir)?;
        json(dir.join("native.json"), &self.metadata)?;
        fs::write(dir.join("evidence.bin"), self.evidence.bytes())?;
        fs::write(dir.join("ffi-exit.txt"), format!("{}\n", self.code))?;
        if self.code != 0 || self.evidence.copied != 1 {
            return Ok(());
        }
        for (name, words) in [
            ("before", &self.before),
            ("after", &self.after),
            ("first-before", &self.first_before),
            ("first-after", &self.first_after),
            ("seen-half", &self.seen_half),
            ("export-half", &self.export_half),
        ] {
            fs::write(dir.join(format!("{name}.u16.bin")), u16_bytes(words))?;
        }
        for (name, words) in [("seen", &self.seen), ("export-float", &self.export_float)] {
            fs::write(dir.join(format!("{name}.u32.bin")), u32_bytes(words))?;
        }
        Ok(())
    }
    fn load(dir: &Path, c: Config, n: usize) -> io::Result<Self> {
        let first_n = if c.mrt() { n * 4 } else { 0 };
        Ok(Self {
            evidence: Evidence::read(&dir.join("evidence.bin"))?,
            code: fs::read_to_string(dir.join("ffi-exit.txt"))?
                .trim()
                .parse()
                .map_err(|_| err("invalid ffi exit"))?,
            metadata: fs::read_to_string(dir.join("native.json"))?,
            before: read16(dir.join("before.u16.bin"), n * c.channels())?,
            after: read16(dir.join("after.u16.bin"), n * c.channels())?,
            first_before: read16(dir.join("first-before.u16.bin"), first_n)?,
            first_after: read16(dir.join("first-after.u16.bin"), first_n)?,
            seen: read32(dir.join("seen.u32.bin"), n * 4)?,
            seen_half: read16(dir.join("seen-half.u16.bin"), n * 4)?,
            export_float: read32(dir.join("export-float.u32.bin"), n * 4)?,
            export_half: read16(dir.join("export-half.u16.bin"), n * 4)?,
        })
    }
}
#[derive(Default)]
struct Checks {
    items: Vec<(&'static str, usize)>,
}
impl Checks {
    fn add(&mut self, label: &'static str, count: usize) {
        if count > 0 {
            self.items.push((label, count));
        }
    }
    fn ok(&self) -> bool {
        self.items.is_empty()
    }
    fn has(&self, name: &str) -> bool {
        self.items.iter().any(|(k, _)| *k == name)
    }
    fn json(&self) -> String {
        object(
            &self
                .items
                .iter()
                .map(|(k, v)| (*k, v.to_string()))
                .collect::<Vec<_>>(),
        )
    }
}
fn check(c: Config, f: &Fixture, r: &Record, grid: &Lattice) -> Checks {
    let mut out = Checks::default();
    let e = &r.evidence;
    let n = f.count();
    let ch = c.channels();
    out.add(
        "native_completion",
        usize::from(
            r.code != 0
                || e.abi != 1
                || e.command_status != 4
                || e.completed != 1
                || e.no_error != 1
                || e.copied != 1
                || e.guards != 1
                || e.device_id == 0,
        ),
    );
    out.add(
        "native_descriptor",
        usize::from(
            e.channels as usize != ch
                || e.layout != c.layout
                || e.route != c.route
                || e.enabled != c.enabled
                || e.mask != c.mask
                || e.variant != c.variant
                || e.count as usize != n
                || e.dst_factor != 0,
        ),
    );
    out.add("native_metadata", usize::from(r.metadata.trim().is_empty()));
    out.add(
        "length",
        usize::from(
            r.before.len() != n * ch
                || r.after.len() != n * ch
                || r.seen.len() != n * 4
                || r.seen_half.len() != n * 4
                || r.export_float.len() != n * 4
                || r.export_half.len() != n * 4
                || r.first_before.len() != if c.mrt() { n * 4 } else { 0 }
                || r.first_after.len() != if c.mrt() { n * 4 } else { 0 },
        ),
    );
    if out.has("length") {
        return out;
    }
    out.add(
        "source_echo",
        r.seen.iter().zip(&f.src).filter(|(a, b)| a != b).count(),
    );
    out.add(
        "half_echo",
        r.seen_half
            .iter()
            .zip(&f.raw)
            .filter(|(a, b)| a != b)
            .count(),
    );
    let mut export_bad = 0;
    let mut seed_bad = 0;
    let mut masked = 0;
    let mut anchors = 0;
    for i in 0..n * 4 {
        let wanted = match c.route {
            0 => f.src[i],
            1 | 2 => widen(f.raw[i]),
            3 => widen(r.export_half[i]),
            _ => unreachable!(),
        };
        export_bad += usize::from(r.export_float[i] != wanted);
        export_bad += usize::from(match c.route {
            0 => r.export_half[i] != 0xa55a,
            1 | 2 => r.export_half[i] != f.raw[i],
            _ => false,
        });
    }
    for p in 0..n {
        for channel in 0..ch {
            let k = p * ch + channel;
            let s = p * 4 + channel;
            seed_bad += usize::from(r.before[k] != f.dst[s]);
            if !c.written(channel) {
                masked += usize::from(r.after[k] != f.dst[s]);
            } else {
                let h = grid.convert(r.export_float[s], true);
                // Exact, nonzero normal source over a finite destination is an
                // arithmetic sanity anchor, not a rounding or NaN propagation oracle.
                if h & 0x7c00 != 0
                    && h & 0x7c00 != 0x7c00
                    && widen(h) == r.export_float[s]
                    && f.dst[s] & 0x7c00 != 0x7c00
                {
                    anchors += usize::from(r.after[k] != h);
                }
            }
        }
    }
    if c.mrt() {
        for i in 0..n * 4 {
            seed_bad += usize::from(r.first_before[i] != f.dst[i]);
            if c.layout == 3 {
                masked += usize::from(r.first_after[i] != f.dst[i]);
            }
        }
    }
    out.add("export_sidecar", export_bad);
    out.add("raw_seed", seed_bad);
    out.add("masked_bytes", masked);
    out.add("finite_exact_anchor", anchors);
    out
}

fn write_corpus(root: &Path) -> io::Result<()> {
    new_dir(root)?;
    let mut csv = String::from("route,pool_index,kind,f32_bits,companion_half_bits\n");
    for route in 0..4 {
        let f = Fixture::new(route);
        for (i, v) in f.pool.iter().enumerate() {
            csv.push_str(&format!(
                "{},{i},{},{:08x},{:04x}\n",
                ROUTES[route as usize], v.kind, v.bits, v.raw
            ));
        }
        let name = ROUTES[route as usize];
        fs::write(
            root.join(format!("{name}-source.u32.bin")),
            u32_bytes(&f.src),
        )?;
        fs::write(root.join(format!("{name}-half.u16.bin")), u16_bytes(&f.raw))?;
        fs::write(
            root.join(format!("{name}-destination.u16.bin")),
            u16_bytes(&f.dst),
        )?;
    }
    fs::write(root.join("values.csv"), csv)?;
    json(root.join("contract.json"),&object(&[
        ("schema","1".into()),("backend",quote("portable_generated_inputs_not_gpu_results")),
        ("width","16".into()),("configuration_count",configurations().len().to_string()),("native_negative_controls","3".into()),
        ("destination_words",array(DEST.iter().map(|x|quote(&format!("{x:04x}"))))),
        ("routes",array(ROUTES.iter().map(|x|quote(x)))),("layouts",array(LAYOUTS.iter().map(|x|quote(x)))),
        ("source_pool_lengths",array((0..4).map(|r|pool(r).len().to_string()))),
        ("hypotheses",array([quote("round_to_nearest_ties_to_even"),quote("round_toward_zero")])),
        ("semantics",quote("hypotheses are not Metal oracles; raw native results and pre-export sidecars retained")),
        ("half_words",quote("prequantized finite half bit patterns, never CPU casts of midpoint values")),
        ("output_layout",quote("little-endian packed words, row-major, per-pixel RG or RGBA; sidecars always four channels")),
        ("coverage",quote("every source crossed with every destination in every stored channel; Red and All masks; MRT color0 blend disabled with All or None mask")),
    ]))
}

struct Report {
    text: String,
    pairs: String,
    examples: String,
    passed: bool,
}
fn report(root: &Path) -> io::Result<Report> {
    let grid = Lattice::new();
    let mut records = Vec::new();
    let mut integrity = Vec::new();
    let mut passed = true;
    let configs = configurations();
    for &c in &configs {
        let f = Fixture::new(c.route);
        let r = Record::load(&root.join("cases").join(c.name()), c, f.count())?;
        let checked = check(c, &f, &r, &grid);
        passed &= checked.ok();
        integrity.push(object(&[
            ("case", quote(&c.name())),
            ("violations", checked.json()),
        ]));
        records.push(r);
    }
    let mut neg = Vec::new();
    for c in controls() {
        let f = Fixture::new(c.route);
        let r = Record::load(&root.join("controls").join(c.name()), c, f.count())?;
        let checks = check(c, &f, &r, &grid);
        let detected = match c.variant {
            1 => checks.has("finite_exact_anchor"),
            2 => checks.has("masked_bytes"),
            3 => checks.has("finite_exact_anchor") && r.after[10 * 2] == 0x4000,
            _ => false,
        };
        passed &= detected;
        neg.push(object(&[
            ("case", quote(&c.name())),
            ("rejected", (!checks.ok()).to_string()),
            ("specific_fault_detected", detected.to_string()),
            ("violations", checks.json()),
            (
                "source_one_destination_one_result",
                quote(&format!("{:04x}", r.after[20])),
            ),
        ]));
    }
    let mut pairs=String::from("route,layout,mask,pixels,written_words,off_on_different,masked_different,color0_different,off_rne_matches,off_rtz_matches,on_rne_matches,on_rtz_matches,on_destination_dependent_words,export_sidecar_pair_differences,cast_rne_matches,cast_rtz_matches,cast_words\n");
    let mut examples=String::from("case,pixel,channel,source_kind,source_f32,source_half,export_f32,export_half,destination,off,on,rne,rtz\n");
    let mut total_words = 0;
    let mut total_diff = 0;
    let mut pairs_json = Vec::new();
    let mut c0_baseline_diff = 0;
    for index in (0..configs.len()).step_by(2) {
        let c = configs[index];
        let f = Fixture::new(c.route);
        let off = &records[index];
        let on = &records[index + 1];
        let ch = c.channels();
        let mut compared = 0;
        let mut diff = 0;
        let mut masked_diff = 0;
        let mut off_rne = 0;
        let mut off_rtz = 0;
        let mut on_rne = 0;
        let mut on_rtz = 0;
        let mut dst_dep = 0;
        let mut shown = 0;
        for p in 0..f.count() {
            for channel in 0..ch {
                let k = p * ch + channel;
                let s = p * 4 + channel;
                if !c.written(channel) {
                    masked_diff += usize::from(off.after[k] != on.after[k]);
                    continue;
                }
                let rne = grid.convert(on.export_float[s], true);
                let rtz = grid.convert(on.export_float[s], false);
                compared += 1;
                diff += usize::from(off.after[k] != on.after[k]);
                off_rne += usize::from(off.after[k] == grid.convert(off.export_float[s], true));
                off_rtz += usize::from(off.after[k] == grid.convert(off.export_float[s], false));
                on_rne += usize::from(on.after[k] == rne);
                on_rtz += usize::from(on.after[k] == rtz);
                let zero_col = (16 - 3 * channel) % 16;
                let base_pixel = (p / 16) * 16 + zero_col;
                dst_dep += usize::from(on.after[k] != on.after[base_pixel * ch + channel]);
                if off.after[k] != on.after[k] && shown < 64 {
                    examples.push_str(&format!("{},{p},{channel},{},{:08x},{:04x},{:08x},{:04x},{:04x},{:04x},{:04x},{rne:04x},{rtz:04x}\n",
                    c.name(),f.value(p,channel).kind,f.src[s],f.raw[s],on.export_float[s],on.export_half[s],f.dst[s],off.after[k],on.after[k]));
                    shown += 1;
                }
            }
        }
        let first_diff = off
            .first_after
            .iter()
            .zip(&on.first_after)
            .filter(|(a, b)| a != b)
            .count();
        let sidecar_diff = off
            .export_float
            .iter()
            .zip(&on.export_float)
            .filter(|(a, b)| a != b)
            .count()
            + off
                .export_half
                .iter()
                .zip(&on.export_half)
                .filter(|(a, b)| a != b)
                .count();
        // A changed unmasked color0 or cast sidecar would be an observation of
        // compilation/context, not evidence to discard or an assumed equality.
        passed &= masked_diff == 0;
        let mut cast_rne = 0;
        let mut cast_rtz = 0;
        if c.route == 3 {
            for s in 0..f.src.len() {
                cast_rne += usize::from(on.export_half[s] == grid.convert(f.src[s], true));
                cast_rtz += usize::from(on.export_half[s] == grid.convert(f.src[s], false));
            }
        }
        let cast_n = if c.route == 3 { f.src.len() } else { 0 };
        pairs.push_str(&format!("{},{},{},{},{compared},{diff},{masked_diff},{first_diff},{off_rne},{off_rtz},{on_rne},{on_rtz},{dst_dep},{sidecar_diff},{cast_rne},{cast_rtz},{cast_n}\n",
            ROUTES[c.route as usize],LAYOUTS[c.layout as usize],c.mask,f.count()));
        total_words += compared;
        total_diff += diff;
        pairs_json.push(object(&[
            ("case", quote(&c.name())),
            ("written_words", compared.to_string()),
            ("off_on_different", diff.to_string()),
            ("on_destination_dependent_words", dst_dep.to_string()),
            ("off_rne_matches", off_rne.to_string()),
            ("off_rtz_matches", off_rtz.to_string()),
            ("on_rne_matches", on_rne.to_string()),
            ("on_rtz_matches", on_rtz.to_string()),
            ("color0_pair_differences", first_diff.to_string()),
            ("export_sidecar_pair_differences", sidecar_diff.to_string()),
        ]));
        if c.layout == 2 {
            let base = configs
                .iter()
                .position(|q| q.route == c.route && q.layout == 1 && q.mask == 15 && q.enabled == 0)
                .unwrap();
            c0_baseline_diff += off
                .first_after
                .iter()
                .zip(&records[base].after)
                .filter(|(a, b)| a != b)
                .count();
            c0_baseline_diff += on
                .first_after
                .iter()
                .zip(&records[base].after)
                .filter(|(a, b)| a != b)
                .count();
        }
    }
    // Cross-format/context differences are observations, not transport failures.
    let text=object(&[("schema","1".into()),("backend",quote("native_metal_readback")),("integrity_and_controls_passed",passed.to_string()),
        ("normal_cases",configs.len().to_string()),("pair_count",(configs.len()/2).to_string()),
        ("written_words_compared",total_words.to_string()),("off_on_different_words",total_diff.to_string()),
        ("mrt_color0_vs_single_rgba_differences",c0_baseline_diff.to_string()),
        ("hypotheses_are_oracles","false".into()),("scope",quote("observed devices and exact OS/compiler builds only; no general Metal or adjacent-build equivalence claim")),
        ("integrity",array(integrity)),("negative_controls",array(neg)),("pairs",array(pairs_json)),
        ("example_limit_per_pair","64".into()),("all_exact_bits_retained_in_binary_files","true".into())]);
    Ok(Report {
        text,
        pairs,
        examples,
        passed,
    })
}

#[cfg(blend_native)]
extern "C" {
    fn blend_native(
        layout: u32,
        route: u32,
        enabled: u32,
        mask: u32,
        variant: u32,
        source: *const std::ffi::c_char,
        src: *const u32,
        raw: *const u16,
        dst: *const u16,
        count: usize,
        before: *mut u16,
        after: *mut u16,
        first_before: *mut u16,
        first_after: *mut u16,
        seen: *mut u32,
        seen_half: *mut u16,
        export_float: *mut u32,
        export_half: *mut u16,
        e: *mut Evidence,
        json: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
}
#[cfg(blend_native)]
fn native(c: Config, f: &Fixture) -> io::Result<Record> {
    let source = std::ffi::CString::new(include_str!("probe.metal")).unwrap();
    let mut r = Record::empty(c, f.count());
    let mut text = vec![0u8; 65536];
    r.code = unsafe {
        blend_native(
            c.layout,
            c.route,
            c.enabled,
            c.mask,
            c.variant,
            source.as_ptr(),
            f.src.as_ptr(),
            f.raw.as_ptr(),
            f.dst.as_ptr(),
            f.count(),
            r.before.as_mut_ptr(),
            r.after.as_mut_ptr(),
            r.first_before.as_mut_ptr(),
            r.first_after.as_mut_ptr(),
            r.seen.as_mut_ptr(),
            r.seen_half.as_mut_ptr(),
            r.export_float.as_mut_ptr(),
            r.export_half.as_mut_ptr(),
            &mut r.evidence,
            text.as_mut_ptr().cast(),
            text.len(),
        )
    };
    let end = text
        .iter()
        .position(|&x| x == 0)
        .ok_or_else(|| err("unterminated native metadata"))?;
    r.metadata =
        String::from_utf8(text[..end].to_vec()).map_err(|_| err("non-UTF8 native metadata"))?;
    if r.metadata.is_empty() {
        r.metadata = object(&[
            ("status", quote("missing_native_metadata")),
            ("ffi_exit", r.code.to_string()),
        ]);
    }
    Ok(r)
}
#[cfg(blend_native)]
fn observe(root: &Path) -> io::Result<()> {
    new_dir(root)?;
    write_corpus(&root.join("inputs"))?;
    for (sub, configs) in [("cases", configurations()), ("controls", controls())] {
        for c in configs {
            let f = Fixture::new(c.route);
            let r = native(c, &f)?;
            r.save(&root.join(sub).join(c.name()))?;
            println!(
                "{} native_exit={} copied={}",
                c.name(),
                r.code,
                r.evidence.copied
            );
            if r.code != 0 || r.evidence.copied != 1 {
                return Err(err(format!(
                    "native failure in {}; retained exact diagnostic; no fallback",
                    c.name()
                )));
            }
        }
    }
    let r = report(root)?;
    json(root.join("summary.json"), &r.text)?;
    fs::write(root.join("pairs.csv"), r.pairs)?;
    fs::write(root.join("difference-examples.csv"), r.examples)?;
    if !r.passed {
        return Err(err(
            "native checks or negative controls failed; raw observations retained",
        ));
    }
    Ok(())
}
#[cfg(not(blend_native))]
fn observe(_: &Path) -> io::Result<()> {
    Err(err("native macOS binary required; no CPU-as-GPU fallback"))
}

fn run() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        return Err(err(
            "usage: probe plan|observe|check NEW_OR_EXISTING_DIRECTORY",
        ));
    }
    let path = Path::new(&args[2]);
    match args[1].as_str() {
        "plan" => write_corpus(path),
        "observe" => observe(path),
        "check" => {
            let r = report(path)?;
            eprintln!(
                "regenerated_pair_csv_bytes={} regenerated_example_csv_bytes={}",
                r.pairs.len(),
                r.examples.len()
            );
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
    fn test_config() -> Config {
        Config {
            route: 0,
            layout: 2,
            mask: 8,
            enabled: 1,
            variant: 0,
        }
    }
    // TEST ONLY synthetic record; never callable by the observation executable.
    fn synthetic(c: Config, f: &Fixture) -> Record {
        let grid = Lattice::new();
        let mut r = Record::empty(c, f.count());
        r.code = 0;
        r.metadata = "synthetic test fixture, NOT native evidence".into();
        r.evidence = Evidence {
            abi: 1,
            command_status: 4,
            completed: 1,
            no_error: 1,
            copied: 1,
            guards: 1,
            channels: c.channels() as u32,
            layout: c.layout,
            route: c.route,
            enabled: c.enabled,
            mask: c.mask,
            variant: c.variant,
            count: f.count() as u32,
            dst_factor: 0,
            device_id: 1,
        };
        r.seen = f.src.clone();
        r.seen_half = f.raw.clone();
        for i in 0..f.src.len() {
            r.export_half[i] = match c.route {
                0 => 0xa55a,
                1 | 2 => f.raw[i],
                3 => grid.convert(f.src[i], true),
                _ => unreachable!(),
            };
            r.export_float[i] = match c.route {
                0 => f.src[i],
                _ => widen(r.export_half[i]),
            };
        }
        for p in 0..f.count() {
            for channel in 0..c.channels() {
                let k = p * c.channels() + channel;
                let s = p * 4 + channel;
                r.before[k] = f.dst[s];
                r.after[k] = if c.written(channel) {
                    grid.convert(r.export_float[s], true)
                } else {
                    f.dst[s]
                };
            }
        }
        if c.mrt() {
            r.first_before = f.dst.clone();
            r.first_after = if c.layout == 3 {
                f.dst.clone()
            } else {
                r.export_float
                    .iter()
                    .map(|&b| grid.convert(b, true))
                    .collect()
            };
        }
        r
    }
    #[test]
    fn abi_layout() {
        assert_eq!(std::mem::size_of::<Evidence>(), 64);
        assert_eq!(Evidence::default().bytes().len(), 64);
    }
    #[test]
    fn widen_known_bits() {
        for (h, b) in [
            (0, 0),
            (0x8000, 0x80000000),
            (1, 0x33800000),
            (0x0400, 0x38800000),
            (0x3c00, 0x3f800000),
            (0x7bff, 0x477fe000),
        ] {
            assert_eq!(widen(h), b);
        }
    }
    #[test]
    fn all_finite_half_words_round_trip_both_hypotheses() {
        let g = Lattice::new();
        for h in 0..=u16::MAX {
            if h & 0x7c00 != 0x7c00 {
                assert_eq!(g.convert(widen(h), true), h);
                assert_eq!(g.convert(widen(h), false), h);
            }
        }
    }
    #[test]
    fn every_finite_midpoint_and_f32_neighbors() {
        let g = Lattice::new();
        for h in 0..0x7bffu16 {
            let mid = ((g.0[h as usize] + g.0[h as usize + 1]) / 2.0) as f32;
            let bits = mid.to_bits();
            for sign in [0u32, 0x80000000] {
                let sh = (sign >> 16) as u16;
                assert_eq!(g.convert((bits - 1) | sign, true), h | sh);
                assert_eq!(
                    g.convert(bits | sign, true),
                    (if h % 2 == 0 { h } else { h + 1 }) | sh
                );
                assert_eq!(g.convert((bits + 1) | sign, true), (h + 1) | sh);
                assert_eq!(g.convert((bits + 1) | sign, false), h | sh);
            }
        }
    }
    #[test]
    fn hypotheses_diverge_and_preserve_signed_zero() {
        let g = Lattice::new();
        assert_eq!(g.convert(0x3f801001, true), 0x3c01);
        assert_eq!(g.convert(0x3f801001, false), 0x3c00);
        assert_eq!(g.convert(0x80000000, true), 0x8000);
        assert_eq!(g.convert(f32::MAX.to_bits(), true), 0x7c00);
        assert_eq!(g.convert(f32::MAX.to_bits(), false), 0x7bff);
    }
    #[test]
    fn corpus_is_finite_bounded_and_crossed() {
        for route in 0..4 {
            let f = Fixture::new(route);
            assert!(f.count() <= 4096);
            assert_eq!(f.count() % 16, 0);
            assert!(f.src.iter().all(|&b| f32::from_bits(b).is_finite()));
            for channel in 0..4 {
                for row in 0..f.pool.len() {
                    let mut found = Vec::new();
                    for col in 0..16 {
                        let k = (row * 16 + col) * 4 + channel;
                        found.push(f.dst[k]);
                        assert_eq!(f.src[k], f.value(row * 16 + col, channel).bits);
                    }
                    found.sort();
                    let mut want = DEST.to_vec();
                    want.sort();
                    assert_eq!(found, want);
                }
            }
        }
        assert_eq!(configurations().len(), 64);
    }
    #[test]
    fn synthetic_positive_is_not_gpu_evidence() {
        let c = test_config();
        let f = Fixture::new(c.route);
        assert!(check(c, &f, &synthetic(c, &f), &Lattice::new()).ok());
    }
    #[test]
    fn negative_written_exact_anchor() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.after[0] ^= 0x8000;
        assert!(check(c, &f, &r, &Lattice::new()).has("finite_exact_anchor"));
    }
    #[test]
    fn negative_masked_payload() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.after[1] ^= 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("masked_bytes"));
    }
    #[test]
    fn negative_source_echo() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.seen[0] ^= 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("source_echo"));
    }
    #[test]
    fn negative_half_echo() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.seen_half[0] ^= 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("half_echo"));
    }
    #[test]
    fn negative_export_sidecar() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.export_float[0] ^= 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("export_sidecar"));
    }
    #[test]
    fn negative_seed_payload() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.before[0] ^= 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("raw_seed"));
    }
    #[test]
    fn negative_completion() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.evidence.completed = 0;
        assert!(check(c, &f, &r, &Lattice::new()).has("native_completion"));
    }
    #[test]
    fn negative_mask_descriptor() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.evidence.mask = 15;
        assert!(check(c, &f, &r, &Lattice::new()).has("native_descriptor"));
    }
    #[test]
    fn negative_blend_factor_descriptor() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.evidence.dst_factor = 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("native_descriptor"));
    }
    #[test]
    fn negative_truncated_words() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.after.pop();
        assert!(check(c, &f, &r, &Lattice::new()).has("length"));
    }
    #[test]
    fn negative_missing_metadata() {
        let c = test_config();
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.metadata.clear();
        assert!(check(c, &f, &r, &Lattice::new()).has("native_metadata"));
    }
    #[test]
    fn negative_fully_masked_mrt_bytes() {
        let mut c = test_config();
        c.layout = 3;
        let f = Fixture::new(c.route);
        let mut r = synthetic(c, &f);
        r.first_after[1] ^= 1;
        assert!(check(c, &f, &r, &Lattice::new()).has("masked_bytes"));
    }
}
