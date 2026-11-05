use std::{env, fs, io, path::Path, process::Command};
const VERSION: &str = "generic-half-clamp-1";
const SOURCE: &str = include_str!("probe.metal");
const SOURCES: [(&str, &[u8]); 5] = [
    ("scripts/clamp/host.rs", include_bytes!("host.rs")),
    ("scripts/clamp/bridge.m", include_bytes!("bridge.m")),
    ("scripts/clamp/probe.metal", include_bytes!("probe.metal")),
    ("scripts/clamp/run.sh", include_bytes!("run.sh")),
    (
        ".github/workflows/clamp.yml",
        include_bytes!("../../.github/workflows/clamp.yml"),
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
#[derive(Clone, Copy, Debug)]
struct Case {
    a: u16,
    b: u16,
    lo: u16,
    hi: u16,
    route: u16,
}
impl Case {
    fn words(self) -> [u16; 5] {
        [self.a, self.b, self.lo, self.hi, self.route]
    }
}
fn case(a: u16, b: u16, lo: u16, hi: u16, route: u16) -> Case {
    Case {
        a,
        b,
        lo,
        hi,
        route,
    }
}
fn corpus(nan_group: bool) -> Vec<Case> {
    if nan_group {
        return vec![
            case(0x7e01, 0x3c00, 0, 0x3c00, 0),
            case(0xfe55, 0x3c00, 0, 0x3c00, 0),
            case(0x7fff, 0x3c00, 0xb800, 0x3800, 0),
            case(0xfe01, 0x3c00, 0xc000, 0x4200, 0),
            case(0, 0, 0, 0x3c00, 1),
            case(0, 0x8000, 0, 0x3c00, 1),
            case(0x8000, 0, 0xb800, 0x3800, 1),
            case(0x8000, 0x8000, 0xc000, 0x4200, 1),
        ];
    }
    let mut v = Vec::new();
    for (lo, hi) in [(0, 0x3c00), (0xb800, 0x3800), (0xc000, 0x4200)] {
        v.push(case(0x7c00, 0x3c00, lo, hi, 0));
        v.push(case(0xfc00, 0x3c00, lo, hi, 0));
        for a in [0x3400, 0xb400] {
            for b in [0, 0x8000] {
                v.push(case(a, b, lo, hi, 1));
            }
        }
    }
    for a in [0xbc00, 0x3400, 0x3800, 0x4000] {
        v.push(case(a, 0x3c00, 0, 0x3c00, 0));
    }
    for a in [0xbc00, 0xb400, 0, 0x8000, 0x3400, 0x3c00] {
        v.push(case(a, 0x3c00, 0xb800, 0x3800, 0));
    }
    for a in [0xc200, 0xc000, 0x3c00, 0x4400] {
        v.push(case(a, 0x3c00, 0xc000, 0x4200, 0));
    }
    v
}
fn count(g: bool) -> usize {
    if g {
        8
    } else {
        32
    }
}
fn input_words(g: bool) -> Vec<u16> {
    corpus(g).iter().flat_map(|c| c.words()).collect()
}
fn is_nan(h: u16) -> bool {
    h & 0x7c00 == 0x7c00 && h & 1023 != 0
}
fn is_inf(h: u16) -> bool {
    h & 0x7fff == 0x7c00
}
fn is_finite(h: u16) -> bool {
    h & 0x7c00 != 0x7c00
}
fn decode(h: u16) -> f64 {
    let s = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
    let e = (h >> 10) & 31;
    let f = h & 1023;
    s * match e {
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
fn valid_bounds(c: Case) -> bool {
    is_finite(c.lo) && is_finite(c.hi) && decode(c.lo) <= decode(c.hi)
}
// Closed CPU reference for this corpus only; it never supplies native outputs.
fn expected_mid(c: Case) -> Option<u16> {
    if c.route == 0 {
        return if is_nan(c.a) { None } else { Some(c.a) };
    }
    if c.route == 1 && is_finite(c.a) && c.a & 0x7fff != 0 && c.b & 0x7fff == 0 {
        return Some(((c.a ^ c.b) & 0x8000) | 0x7c00);
    }
    None
}
fn expected_result(c: Case) -> Option<u16> {
    if !valid_bounds(c) {
        return None;
    }
    let x = expected_mid(c)?;
    let value = decode(x);
    Some(if value < decode(c.lo) {
        c.lo
    } else if value > decode(c.hi) {
        c.hi
    } else {
        x
    })
}
fn class(h: u16) -> &'static str {
    if is_nan(h) {
        "nan"
    } else if is_inf(h) {
        if h & 0x8000 == 0 {
            "positive_inf"
        } else {
            "negative_inf"
        }
    } else if h & 0x7fff == 0 {
        if h == 0 {
            "positive_zero"
        } else {
            "negative_zero"
        }
    } else {
        "finite"
    }
}
#[derive(Clone)]
struct Observation {
    echo: Vec<u16>,
    mid: Vec<u16>,
    result: Vec<u16>,
}
impl Observation {
    fn sentinel(g: bool) -> Self {
        Self {
            echo: vec![0xa5a5; count(g) * 5],
            mid: vec![0xa5a5; count(g)],
            result: vec![0xa5a5; count(g)],
        }
    }
    fn shape(&self, g: bool) -> bool {
        self.echo.len() == count(g) * 5
            && self.mid.len() == count(g)
            && self.result.len() == count(g)
    }
    fn save(&self, out: &Path, key: &str) -> io::Result<()> {
        for (name, v) in [
            ("echo", &self.echo),
            ("intermediate", &self.mid),
            ("result", &self.result),
        ] {
            fs::write(out.join(format!("{key}.{name}.bin")), half_bytes(v))?;
        }
        Ok(())
    }
    fn load(out: &Path, key: &str) -> io::Result<Self> {
        fn read(p: &Path) -> io::Result<Vec<u16>> {
            if fs::metadata(p)?.len() > 4096 {
                return Err(io::Error::other("oversized observation"));
            }
            let b = fs::read(p)?;
            if b.len() % 2 != 0 {
                return Err(io::Error::other("unaligned observation"));
            }
            Ok(b.chunks_exact(2)
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
                .collect())
        }
        Ok(Self {
            echo: read(&out.join(format!("{key}.echo.bin")))?,
            mid: read(&out.join(format!("{key}.intermediate.bin")))?,
            result: read(&out.join(format!("{key}.result.bin")))?,
        })
    }
}
struct Check {
    pass: bool,
    json: String,
}
fn check(o: &Observation, g: bool) -> Check {
    let shape = o.shape(g);
    let cases = corpus(g);
    let input = input_words(g);
    let echoes = input.iter().zip(&o.echo).filter(|(a, b)| a != b).count();
    let mut mid_bad = 0;
    let mut out_bad = 0;
    let mut direct_inf = 0;
    let mut division_inf = 0;
    let mut nan_mid = 0;
    for (i, c) in cases
        .iter()
        .enumerate()
        .take(o.mid.len().min(o.result.len()))
    {
        if g {
            nan_mid += usize::from(is_nan(o.mid[i]));
        } else {
            mid_bad += usize::from(Some(o.mid[i]) != expected_mid(*c));
            out_bad += usize::from(Some(o.result[i]) != expected_result(*c));
            if is_inf(o.mid[i]) {
                if c.route == 0 {
                    direct_inf += 1;
                } else {
                    division_inf += 1;
                }
            }
        }
    }
    let pass = shape
        && echoes == 0
        && if g {
            nan_mid == 8
        } else {
            mid_bad == 0 && out_bad == 0 && direct_inf == 6 && division_inf == 12
        };
    Check {
        pass,
        json: obj(&[
            ("shape_valid", shape.to_string()),
            ("echo_word_mismatches", echoes.to_string()),
            (
                "intermediate_mismatches",
                if g {
                    "null".into()
                } else {
                    mid_bad.to_string()
                },
            ),
            (
                "clamped_result_mismatches",
                if g {
                    "null".into()
                } else {
                    out_bad.to_string()
                },
            ),
            ("direct_infinities_observed", direct_inf.to_string()),
            ("division_infinities_observed", division_inf.to_string()),
            ("nan_intermediates_observed", nan_mid.to_string()),
            ("nan_results_observation_only", g.to_string()),
            ("nan_admission_qualified", "false".into()),
            ("checks_pass", pass.to_string()),
        ]),
    }
}
fn exact_negative(p: &Observation, n: &Observation) -> bool {
    p.shape(false)
        && n.shape(false)
        && p.echo == input_words(false)
        && n.echo == p.echo
        && n.mid == p.mid
        && p.result[0] == 0x3c00
        && n.result[0] == 0
        && n.result[1..] == p.result[1..]
}
fn table(o: &Observation, g: bool) -> String {
    let mut text=String::from("case,route,a_half,b_half,lower_half,upper_half,intermediate_half,intermediate_class,result_half,result_class,expected_intermediate,expected_result\n");
    for (i, c) in corpus(g).iter().enumerate() {
        let m = expected_mid(*c).map_or("observation".into(), |x| format!("0x{x:04x}"));
        let y = if g {
            "observation".into()
        } else {
            expected_result(*c).map_or("invalid".into(), |x| format!("0x{x:04x}"))
        };
        text.push_str(&format!(
            "{i},{},0x{:04x},0x{:04x},0x{:04x},0x{:04x},0x{:04x},{},0x{:04x},{},{m},{y}\n",
            c.route,
            c.a,
            c.b,
            c.lo,
            c.hi,
            o.mid[i],
            class(o.mid[i]),
            o.result[i],
            class(o.result[i])
        ));
    }
    text
}
#[repr(C)]
#[derive(Default)]
struct Evidence {
    abi: u32,
    status: u32,
    completed: u32,
    no_error: u32,
    copied: u32,
    count: u32,
    variant: u32,
    guards: u32,
    device_id: u64,
}
impl Evidence {
    fn valid(&self, g: bool, v: u32) -> bool {
        self.abi == 1
            && self.status == 4
            && self.completed == 1
            && self.no_error == 1
            && self.copied == 1
            && self.count as usize == count(g)
            && self.variant == v
            && self.guards == 1
            && self.device_id != 0
    }
}
#[cfg(all(clamp_native, target_os = "macos"))]
extern "C" {
    fn clamp_native(
        input: *const u16,
        count: usize,
        variant: u32,
        source: *const std::ffi::c_char,
        echo: *mut u16,
        mid: *mut u16,
        result: *mut u16,
        e: *mut Evidence,
        json: *mut std::ffi::c_char,
        cap: usize,
    ) -> i32;
}
fn native(g: bool, v: u32, o: &mut Observation) -> (i32, Evidence, String) {
    #[cfg(all(clamp_native, target_os = "macos"))]
    {
        let inputs = input_words(g);
        let source = std::ffi::CString::new(SOURCE).unwrap();
        let mut e = Evidence::default();
        let mut json = vec![0u8; 16384];
        let rc = unsafe {
            clamp_native(
                inputs.as_ptr(),
                count(g),
                v,
                source.as_ptr(),
                o.echo.as_mut_ptr(),
                o.mid.as_mut_ptr(),
                o.result.as_mut_ptr(),
                &mut e,
                json.as_mut_ptr().cast(),
                json.len(),
            )
        };
        let len = json.iter().position(|b| *b == 0).unwrap_or(json.len());
        let text = String::from_utf8_lossy(&json[..len]).into_owned();
        (
            rc,
            e,
            if text.starts_with('{') {
                text
            } else {
                "{\"status\":\"missing_native_json\"}".into()
            },
        )
    }
    #[cfg(not(all(clamp_native, target_os = "macos")))]
    {
        let _ = (g, v, o, SOURCE);
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
    for (g, label) in [(false, "defined"), (true, "nan")] {
        let cases = corpus(g);
        assert_eq!(cases.len(), count(g));
        assert!(cases.iter().copied().all(valid_bounds));
        fs::write(
            out.join(format!("{label}.input.u16le.bin")),
            half_bytes(&input_words(g)),
        )?;
        if !g {
            let mid: Vec<_> = cases.iter().map(|c| expected_mid(*c).unwrap()).collect();
            let result: Vec<_> = cases.iter().map(|c| expected_result(*c).unwrap()).collect();
            fs::write(
                out.join("defined.expected_intermediate.bin"),
                half_bytes(&mid),
            )?;
            fs::write(out.join("defined.expected_result.bin"), half_bytes(&result))?;
        }
    }
    write_json(out.join("corpus.json"),&obj(&[("version",quote(VERSION)),("defined_cases","32".into()),("nan_cases","8".into()),
  ("record_layout",quote("five little-endian uint16 words: a,b,lower,upper,route; route0 direct, route1 half division")),("bounds",quote("[0,1],[-0.5,0.5],[-2,3]; finite ordered exact half bounds")),
  ("negative",quote("only case0 clamped endpoint 0x3c00 becomes 0x0000; all intermediate, operand echoes and other results unchanged")),("nan_policy",quote("intermediate class checked, final bits observational only; no software admission claim"))]))
}
fn observe(out: &Path) -> io::Result<i32> {
    prepare(out)?;
    let mut positive = None;
    let mut positive_device = 0;
    let mut valid_ids = Vec::new();
    let mut qualified = [false; 3];
    let mut records = Vec::new();
    // No helper processes, process enumeration, signals or termination fallback.
    // The manual CI job supplies the hard wall-clock bound for native work.
    for (index, (g, v, key)) in [
        (false, 0, "defined-positive"),
        (false, 1, "defined-negative"),
        (true, 0, "nan-observation"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut o = Observation::sentinel(g);
        let (rc, e, meta) = native(g, v, &mut o);
        let valid = rc == 0 && e.valid(g, v);
        let c = check(&o, g);
        write_json(out.join(format!("{key}.native.json")), &meta)?;
        if valid {
            o.save(out, key)?;
            fs::write(out.join(format!("{key}.cases.csv")), table(&o, g))?;
            fs::write(
                out.join(format!("{key}.device.bin")),
                e.device_id.to_le_bytes(),
            )?;
            valid_ids.push(e.device_id);
        }
        let exact = valid
            && v == 1
            && positive_device == e.device_id
            && positive.as_ref().is_some_and(|p| exact_negative(p, &o));
        let exit = if valid && v == 1 && exact && !c.pass {
            10
        } else if valid && v == 0 && c.pass {
            0
        } else {
            2
        };
        qualified[index] = exit == if v == 1 { 10 } else { 0 };
        let receipt = obj(&[
            ("native_return_code", rc.to_string()),
            ("gpu_readback_valid", valid.to_string()),
            ("exit_code", exit.to_string()),
            ("exact_negative_control", exact.to_string()),
            ("checks", c.json),
        ]);
        write_json(out.join(format!("{key}.check.json")), &receipt)?;
        records.push(obj(&[("case", quote(key)), ("receipt", receipt)]));
        if index == 0 && valid {
            positive = Some(o);
            positive_device = e.device_id;
        }
    }
    let same =
        valid_ids.len() == 3 && valid_ids[0] != 0 && valid_ids.iter().all(|x| *x == valid_ids[0]);
    write_json(out.join("summary.json"),&obj(&[("version",quote(VERSION)),("defined_infinity_contract_qualified",(qualified[0]&&qualified[1]&&same).to_string()),
  ("nan_observation_complete",qualified[2].to_string()),("nan_admission_qualified","false".into()),("same_device_all_observations",same.to_string()),("completed_device_records",valid_ids.len().to_string()),("cases",array(records)),
  ("scope",quote("Only recorded scalar half-clamp infinity/finite cohort; NaNs observational; no f32, invalid-bound, general nonfinite, SFU or other-build claim."))]))?;
    Ok(if qualified.iter().all(|x| *x) && same {
        0
    } else {
        2
    })
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
                if size > 1024 * 1024 || *total as u64 + size > 4 * 1024 * 1024 || items.len() >= 64
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
fn simulated(g: bool) -> Observation {
    let cases = corpus(g);
    Observation {
        echo: input_words(g),
        mid: cases
            .iter()
            .map(|c| expected_mid(*c).unwrap_or(0x7e00))
            .collect(),
        result: cases
            .iter()
            .map(|c| expected_result(*c).unwrap_or(0x7e00))
            .collect(),
    }
}
fn portable(out: &Path, negative: bool) -> io::Result<i32> {
    prepare(out)?;
    let p = simulated(false);
    let mut n = p.clone();
    n.result[0] = 0;
    let c = check(if negative { &n } else { &p }, false);
    let exact = exact_negative(&p, &n);
    let nan_check = check(&simulated(true), true);
    write_json(
        out.join("portable.json"),
        &obj(&[
            ("gpu_executed", "false".into()),
            ("backend", quote("portable_reference_only")),
            ("negative", negative.to_string()),
            ("exact_wrong_control", exact.to_string()),
            ("checks", c.json),
            ("nan_intermediate_reference_check", nan_check.json),
        ]),
    )?;
    if !exact || !nan_check.pass || c.pass == negative {
        return Ok(1);
    }
    Ok(if negative { 2 } else { 0 })
}
fn entry() -> io::Result<i32> {
    let a: Vec<_> = env::args().collect();
    if a.len() == 4 && a[1] == "seal" {
        return seal(
            Path::new(&a[2]),
            a[3].parse().map_err(|_| io::Error::other("invalid exit"))?,
        );
    }
    if a.len() == 5 && a[1] == "inspect" {
        let g = match a[2].as_str() {
            "defined" => false,
            "nan" => true,
            _ => return Ok(64),
        };
        let o = Observation::load(Path::new(&a[4]), &a[3])?;
        let c = check(&o, g);
        println!("{}", c.json);
        return Ok(if c.pass { 0 } else { 2 });
    }
    if a.len() == 3 {
        match a[1].as_str() {
            "reference" => return portable(Path::new(&a[2]), false),
            "reference-negative" => return portable(Path::new(&a[2]), true),
            "observe" => return observe(Path::new(&a[2])),
            _ => {}
        }
    }
    eprintln!("usage: half-clamp reference|reference-negative|observe NEW_DIRECTORY; inspect defined|nan CASE DIRECTORY; seal DIRECTORY EXIT");
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
    fn corpus_bounds_shapes_and_routes() {
        let c = corpus(false);
        assert_eq!(c.len(), 32);
        assert!(c.iter().copied().all(valid_bounds));
        assert_eq!(c.iter().filter(|c| c.route == 1).count(), 12);
        assert_eq!(c.iter().filter(|c| c.route == 0 && is_inf(c.a)).count(), 6);
        assert_eq!(corpus(true).len(), 8);
        assert!(corpus(true).iter().copied().all(valid_bounds));
        for c in c.iter().filter(|c| c.route == 1) {
            assert!(is_finite(c.a) && c.a & 0x7fff != 0);
            assert_eq!(c.b & 0x7fff, 0);
        }
    }
    #[test]
    fn exact_literal_endpoints_and_division_signs() {
        let c = corpus(false);
        assert_eq!(expected_result(c[0]), Some(0x3c00));
        assert_eq!(expected_result(c[1]), Some(0));
        for (i, want) in [(2, 0x7c00), (3, 0xfc00), (4, 0xfc00), (5, 0x7c00)] {
            assert_eq!(expected_mid(c[i]), Some(want));
        }
        assert_eq!(expected_result(c[6]), Some(0x3800));
        assert_eq!(expected_result(c[7]), Some(0xb800));
        assert_eq!(expected_result(c[12]), Some(0x4200));
        assert_eq!(expected_result(c[13]), Some(0xc000));
        assert_eq!(
            expected_result(case(0x8000, 0x3c00, 0xb800, 0x3800, 0)),
            Some(0x8000)
        );
    }
    #[test]
    fn invalid_bounds_are_never_reference_admission() {
        for c in [
            case(0x7c00, 0, 0x7e00, 0x3c00, 0),
            case(0xfc00, 0, 0, 0x7c00, 0),
            case(0x7c00, 0, 0x3c00, 0, 0),
        ] {
            assert!(!valid_bounds(c));
            assert_eq!(expected_result(c), None);
        }
    }
    #[test]
    fn all_expected_values_are_literal() {
        let p = simulated(false);
        let expected = [
            0x3c00, 0, 0x3c00, 0, 0, 0x3c00, 0x3800, 0xb800, 0x3800, 0xb800, 0xb800, 0x3800,
            0x4200, 0xc000, 0x4200, 0xc000, 0xc000, 0x4200, 0, 0x3400, 0x3800, 0x3c00, 0xb800,
            0xb400, 0, 0x8000, 0x3400, 0x3800, 0xc000, 0xc000, 0x3c00, 0x4200,
        ];
        assert_eq!(p.result, expected);
        assert!(check(&p, false).pass);
    }
    #[test]
    fn rejects_zero_one_pass_through_wrong_sign_and_preclamp_shortcuts() {
        let p = simulated(false);
        let mut b = p.clone();
        b.result.fill(0);
        assert!(!check(&b, false).pass);
        b.result.fill(0x3c00);
        assert!(!check(&b, false).pass);
        b.result = p.mid.clone();
        assert!(!check(&b, false).pass);
        let mut b = p.clone();
        b.mid[3] ^= 0x8000;
        assert!(!check(&b, false).pass);
        let mut b = p.clone();
        b.mid = p.result.clone();
        assert!(
            !check(&b, false).pass,
            "a pre-clamped intermediate must fail"
        );
    }
    #[test]
    fn exact_negative_rejects_every_unrelated_mutation() {
        let p = simulated(false);
        let mut n = p.clone();
        n.result[0] = 0;
        assert!(exact_negative(&p, &n));
        assert!(!check(&n, false).pass);
        assert!(!exact_negative(&p, &p));
        for i in 0..n.mid.len() {
            let mut b = n.clone();
            b.mid[i] ^= 1;
            assert!(!exact_negative(&p, &b));
        }
        for i in 0..n.echo.len() {
            let mut b = n.clone();
            b.echo[i] ^= 1;
            assert!(!exact_negative(&p, &b));
        }
        for i in 1..n.result.len() {
            let mut b = n.clone();
            b.result[i] ^= 1;
            assert!(!exact_negative(&p, &b));
        }
        let mut b = n.clone();
        b.result.pop();
        assert!(!exact_negative(&p, &b));
    }
    #[test]
    fn nan_results_never_become_admission_expectations() {
        let mut o = simulated(true);
        assert!(check(&o, true).pass);
        for v in [0, 0x3c00, 0x7e00, 0xfc00] {
            o.result.fill(v);
            assert!(
                check(&o, true).pass,
                "final NaN case outputs are observations only"
            );
        }
        o.mid[0] = 0;
        assert!(
            !check(&o, true).pass,
            "the GPU must actually expose a NaN intermediate"
        );
        assert!(corpus(true).iter().all(|c| expected_result(*c).is_none()));
    }
    #[test]
    fn evidence_is_closed_and_abi_matches() {
        assert_eq!(std::mem::size_of::<Evidence>(), 40);
        assert!(!Evidence::default().valid(false, 0));
    }
    #[test]
    fn sha_anchor() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
