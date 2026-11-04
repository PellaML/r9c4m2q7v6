use std::{
    env, fs, io,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const VERSION: &str = "generic-unary-1";
const SOURCE: &str = include_str!("probe.metal");
const TOLERANCE: f64 = 0.000004;
const SIN_ONE: f64 = 0.841_470_984_807_896_5;
const COS_ONE: f64 = 0.540_302_305_868_139_8;
const SOURCES: [(&str, &[u8]); 5] = [
    ("scripts/unary/host.rs", include_bytes!("host.rs")),
    ("scripts/unary/bridge.m", include_bytes!("bridge.m")),
    ("scripts/unary/probe.metal", include_bytes!("probe.metal")),
    ("scripts/unary/run.sh", include_bytes!("run.sh")),
    (
        ".github/workflows/unary.yml",
        include_bytes!("../../.github/workflows/unary.yml"),
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
fn number(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.17e}")
    } else {
        "null".into()
    }
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

#[derive(Clone)]
struct Case {
    bits: u32,
    label: String,
}
fn corpus(nonfinite: bool) -> Vec<Case> {
    let mut v: Vec<Case> = Vec::new();
    let mut add = |bits, label: String| {
        if !v.iter().any(|c| c.bits == bits) {
            v.push(Case { bits, label });
        }
    };
    if nonfinite {
        for (bits, name) in [
            (0x7f800000, "positive_infinity"),
            (0xff800000, "negative_infinity"),
            (0x7fc00000, "quiet_nan"),
            (0xffc12345, "negative_quiet_nan_payload"),
            (0x7f800001, "signaling_nan_payload"),
            (0xff800001, "negative_signaling_nan_payload"),
        ] {
            add(bits, name.into());
        }
        return v;
    }
    for (bits, name) in [
        (0, "positive_zero"),
        (0x80000000, "negative_zero"),
        (0x3f800000, "positive_one"),
        (0xbf800000, "negative_one"),
    ] {
        add(bits, name.into());
    }
    for bits in [
        1, 2, 0x00400000, 0x007fffff, 0x00800000, 0x00800001, 0x7f7fffff,
    ] {
        for sign in [0, 0x80000000] {
            add(bits | sign, format!("edge_{:08x}", bits | sign));
        }
    }
    for (center, name) in [
        (0x3f490fdbu32, "pi_over_4"),
        (0x3fc90fdb, "pi_over_2"),
        (0x40490fdb, "pi"),
        (0x40c90fdb, "two_pi"),
        (0x43000000, "radians_128"),
    ] {
        for offset in -3i64..=3 {
            for sign in [0, 0x80000000] {
                add(
                    ((center as i64 + offset) as u32) | sign,
                    format!("{name}_neighbor_{offset}_sign_{sign:08x}"),
                );
            }
        }
    }
    // Small independently generated low-angle grid, not an imported input table.
    // The arithmetic is exact in binary64 before the explicit binary32 rounding.
    let upper = 3.0 * f32::from_bits(0x3ec90fdb) as f64;
    for i in 1..=128 {
        let bits = (upper * i as f64 / 128.0) as f32;
        for sign in [0, 0x80000000] {
            add(
                bits.to_bits() | sign,
                format!("low_angle_grid_{i}_s{sign:08x}"),
            );
        }
    }
    for offset in -3i64..=3 {
        for sign in [0, 0x80000000] {
            add(
                ((0x3ec90fdbi64 + offset) as u32) | sign,
                format!("pi_over_8_neighbor_{offset}_s{sign:08x}"),
            );
        }
    }
    // All normal exponent fields, multiple mantissas and both signs; <=4096 inputs.
    for exponent in 1..=254u32 {
        for mantissa in [0, 1, 0x2aaaaa, 0x7fffff] {
            for sign in [0, 0x80000000] {
                add(
                    sign | (exponent << 23) | mantissa,
                    format!("stratum_e{exponent}_m{mantissa:06x}_s{sign:08x}"),
                );
            }
        }
    }
    v
}
fn input_words(cases: &[Case]) -> Vec<u32> {
    cases.iter().map(|c| c.bits).collect()
}
fn reference(cases: &[Case], variant: u32) -> Vec<u32> {
    cases
        .iter()
        .flat_map(|c| {
            let x = f32::from_bits(c.bits) as f64;
            let s = (x.sin() as f32).to_bits();
            let t = (x.cos() as f32).to_bits();
            [
                c.bits,
                if variant == 1 { t } else { s },
                if variant == 1 {
                    s
                } else if variant == 2 {
                    0
                } else {
                    t
                },
            ]
        })
        .collect()
}
struct Check {
    length_ok: bool,
    echo_ok: bool,
    anchors: usize,
    failures: Vec<String>,
}
impl Check {
    fn anchors_pass(&self) -> bool {
        self.anchors == 4 && self.failures.is_empty()
    }
    fn valid(&self) -> bool {
        self.length_ok && self.echo_ok
    }
    fn json(&self) -> String {
        obj(&[
            ("length_ok", self.length_ok.to_string()),
            ("input_echo_exact", self.echo_ok.to_string()),
            ("anchors_found", self.anchors.to_string()),
            ("known_anchors_passed", self.anchors_pass().to_string()),
            ("absolute_tolerance_at_plus_minus_one", number(TOLERANCE)),
            (
                "anchor_failures",
                array(self.failures.iter().map(|s| quote(s))),
            ),
        ])
    }
}
fn check(cases: &[Case], words: &[u32]) -> Check {
    let mut c = Check {
        length_ok: words.len() == cases.len() * 3,
        echo_ok: true,
        anchors: 0,
        failures: Vec::new(),
    };
    if !c.length_ok {
        c.echo_ok = false;
        return c;
    }
    for (case, row) in cases.iter().zip(words.chunks_exact(3)) {
        c.echo_ok &= row[0] == case.bits;
        let magnitude = case.bits & 0x7fffffff;
        if magnitude == 0 {
            c.anchors += 1;
            if row[1] & 0x7fffffff != 0 {
                c.failures
                    .push(format!("{}:sin_zero_magnitude", case.label));
            }
            if row[2] != 0x3f800000 {
                c.failures.push(format!("{}:cos_zero_not_one", case.label));
            }
        } else if magnitude == 0x3f800000 {
            c.anchors += 1;
            let expected_s = if case.bits >> 31 == 1 {
                -SIN_ONE
            } else {
                SIN_ONE
            };
            let s = f32::from_bits(row[1]) as f64;
            let t = f32::from_bits(row[2]) as f64;
            if !s.is_finite() || (s - expected_s).abs() > TOLERANCE {
                c.failures.push(format!("{}:sin_one_tolerance", case.label));
            }
            if !t.is_finite() || (t - COS_ONE).abs() > TOLERANCE {
                c.failures.push(format!("{}:cos_one_tolerance", case.label));
            }
        }
    }
    c
}
fn observations(cases: &[Case], words: &[u32], nonfinite: bool) -> String {
    array(
        cases
            .iter()
            .zip(words.chunks_exact(3))
            .enumerate()
            .map(|(i, (c, r))| {
                let x = f32::from_bits(c.bits) as f64;
                let diagnostic = if nonfinite {
                    "null".into()
                } else {
                    obj(&[
                        ("host_f64_sin", number(x.sin())),
                        ("host_f64_cos", number(x.cos())),
                        (
                            "sin_absolute_error",
                            number((f32::from_bits(r[1]) as f64 - x.sin()).abs()),
                        ),
                        (
                            "cos_absolute_error",
                            number((f32::from_bits(r[2]) as f64 - x.cos()).abs()),
                        ),
                    ])
                };
                obj(&[
                    ("index", i.to_string()),
                    ("case", quote(&c.label)),
                    ("input_bits", hex(c.bits)),
                    ("echo_bits", hex(r[0])),
                    ("sin_bits", hex(r[1])),
                    ("cos_bits", hex(r[2])),
                    ("host_math_diagnostic_not_oracle", diagnostic),
                ])
            }),
    )
}

#[repr(C)]
#[derive(Default)]
struct Evidence {
    abi_version: u32,
    command_status: u32,
    completed: u32,
    no_error: u32,
    readback_copied: u32,
    fast_math_observed: u32,
    device_registry_id: u64,
}
impl Evidence {
    fn valid(&self, fast: u32) -> bool {
        self.abi_version == 1
            && self.command_status == 4
            && self.completed == 1
            && self.no_error == 1
            && self.readback_copied == 1
            && self.fast_math_observed == fast
            && self.device_registry_id != 0
    }
    fn json(&self) -> String {
        obj(&[
            ("abi_version", self.abi_version.to_string()),
            ("command_status", self.command_status.to_string()),
            ("completed", (self.completed == 1).to_string()),
            ("no_error", (self.no_error == 1).to_string()),
            ("readback_copied", (self.readback_copied == 1).to_string()),
            ("fast_math_observed", self.fast_math_observed.to_string()),
            (
                "device_registry_id",
                quote(&self.device_registry_id.to_string()),
            ),
        ])
    }
}
#[cfg(all(unary_native, target_os = "macos"))]
extern "C" {
    fn unary_native(
        input: *const u32,
        count: usize,
        source: *const std::ffi::c_char,
        fast: u32,
        variant: u32,
        output: *mut u32,
        output_words: usize,
        e: *mut Evidence,
        json: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
}
fn native(input: &[u32], fast: u32, variant: u32, output: &mut [u32]) -> (i32, Evidence, String) {
    #[cfg(all(unary_native, target_os = "macos"))]
    {
        let source = std::ffi::CString::new(SOURCE).unwrap();
        let mut e = Evidence::default();
        let mut json = vec![0u8; 16384];
        let code = unsafe {
            unary_native(
                input.as_ptr(),
                input.len(),
                source.as_ptr(),
                fast,
                variant,
                output.as_mut_ptr(),
                output.len(),
                &mut e,
                json.as_mut_ptr().cast(),
                json.len(),
            )
        };
        let end = json.iter().position(|&b| b == 0).unwrap_or(json.len());
        let text = String::from_utf8_lossy(&json[..end]).into_owned();
        return (
            code,
            e,
            if text.starts_with('{') {
                text
            } else {
                obj(&[("status", quote("missing_native_json"))])
            },
        );
    }
    #[cfg(not(all(unary_native, target_os = "macos")))]
    {
        let _ = (input, fast, variant, output, SOURCE);
        (
            23,
            Evidence::default(),
            obj(&[
                ("status", quote("native_metal_unavailable_no_cpu_fallback")),
                ("gpu_executed", "false".into()),
            ]),
        )
    }
}

fn child(out: &Path, fast: u32, variant: u32, nonfinite: bool, id: &str) -> io::Result<i32> {
    if fast > 1 || variant > 2 || (nonfinite && variant != 0) {
        return Ok(64);
    }
    let cases = corpus(nonfinite);
    let input = input_words(&cases);
    let input_data = bytes(&input);
    let mut output = vec![0xa5a5a5a5u32; input.len() * 3];
    let (code, e, metadata) = native(&input, fast, variant, &mut output);
    write_json(out.join(format!("{id}.native.json")), &metadata)?;
    let gpu_valid = code == 0 && e.valid(fast);
    let checked = check(&cases, if gpu_valid { &output } else { &[] });
    let actual = gpu_valid && checked.valid();
    let negative_rejected =
        actual && variant != 0 && checked.anchors == 4 && !checked.failures.is_empty();
    let accepted = actual
        && if nonfinite {
            true
        } else {
            variant == 0 && checked.anchors_pass()
        };
    if gpu_valid {
        fs::write(out.join(format!("{id}.gpu.bin")), bytes(&output))?;
        fs::write(
            out.join(format!("{id}.device.bin")),
            e.device_registry_id.to_le_bytes(),
        )?;
        write_json(
            out.join(format!("{id}.observations.json")),
            &observations(&cases, &output, nonfinite),
        )?;
    }
    let exit_code = if negative_rejected {
        10
    } else if accepted {
        0
    } else {
        2
    };
    write_json(out.join(format!("{id}.receipt.json")),&obj(&[
        ("probe_version",quote(VERSION)),("native_return_code",code.to_string()),("exit_code",exit_code.to_string()),
        ("fast_math_requested",(fast==1).to_string()),("variant",variant.to_string()),("nonfinite_observation_only",nonfinite.to_string()),
        ("gpu_readback_valid",actual.to_string()),("negative_shader_rejected_by_anchors",negative_rejected.to_string()),
        ("known_anchor_numeric_pass",if nonfinite {"null".into()} else {(accepted && variant==0).to_string()}),
        ("scope",quote("Only known anchors are checked numerically; other finite values are observations. Host math is not Apple SFU bit truth.")),
        ("evidence",e.json()),("checks",checked.json()),("input",digest(if nonfinite {"nonfinite.input.bin"} else {"finite.input.bin"},&input_data)),
        ("output",if gpu_valid {digest(&format!("{id}.gpu.bin"),&bytes(&output))} else {"null".into()}),
        ("shader_source",digest("scripts/unary/probe.metal",SOURCE.as_bytes()))
    ]))?;
    Ok(exit_code)
}
fn prepare(out: &Path) -> io::Result<()> {
    fs::create_dir(out)?;
    for nonfinite in [false, true] {
        let cases = corpus(nonfinite);
        let name = if nonfinite { "nonfinite" } else { "finite" };
        fs::write(
            out.join(format!("{name}.input.bin")),
            bytes(&input_words(&cases)),
        )?;
        write_json(
            out.join(format!("{name}.cases.json")),
            &array(
                cases
                    .iter()
                    .map(|c| obj(&[("label", quote(&c.label)), ("bits", hex(c.bits))])),
            ),
        )?;
    }
    write_json(out.join("sources.json"), &source_receipts())
}
fn portable(out: &Path, variant: u32) -> io::Result<i32> {
    prepare(out)?;
    let cases = corpus(false);
    let words = reference(&cases, variant);
    let checked = check(&cases, &words);
    fs::write(out.join("cpu-reference.bin"), bytes(&words))?;
    write_json(
        out.join("cpu-reference.json"),
        &observations(&cases, &words, false),
    )?;
    let pass = checked.valid() && checked.anchors_pass();
    write_json(
        out.join("summary.json"),
        &obj(&[
            ("probe_version", quote(VERSION)),
            ("backend", quote("portable_host_reference_only")),
            ("gpu_executed", "false".into()),
            ("gpu_probe_qualified", "false".into()),
            ("variant", variant.to_string()),
            ("anchor_checks_passed", pass.to_string()),
            ("checks", checked.json()),
            ("reference", digest("cpu-reference.bin", &bytes(&words))),
            (
                "scope",
                quote("Tests checker plumbing; cannot establish native Metal behavior."),
            ),
        ]),
    )?;
    Ok(if pass { 0 } else { 2 })
}
fn observe(out: &Path) -> io::Result<i32> {
    prepare(out)?;
    let exe = env::current_exe()?;
    let mut results = Vec::new();
    let mut qualified = true;
    let mut devices = Vec::new();
    for fast in 0..=1 {
        for (variant, nonfinite) in [(0, false), (1, false), (2, false), (0, true)] {
            let id = format!(
                "fast-{fast}-variant-{variant}-{}",
                if nonfinite { "nonfinite" } else { "finite" }
            );
            let started = Instant::now();
            let mut child = Command::new(&exe)
                .args([
                    "--child",
                    &fast.to_string(),
                    &variant.to_string(),
                    if nonfinite { "1" } else { "0" },
                ])
                .arg(out)
                .arg(&id)
                .stdout(Stdio::from(fs::File::create(
                    out.join(format!("{id}.stdout.txt")),
                )?))
                .stderr(Stdio::from(fs::File::create(
                    out.join(format!("{id}.stderr.txt")),
                )?))
                .spawn()?;
            let mut timed_out = false;
            let status = loop {
                if let Some(s) = child.try_wait()? {
                    break s;
                }
                if started.elapsed() > Duration::from_secs(45) {
                    timed_out = true;
                    child.kill()?;
                    break child.wait()?;
                }
                thread::sleep(Duration::from_millis(25));
            };
            let expected = if variant == 0 { 0 } else { 10 };
            let passed = !timed_out && status.code() == Some(expected);
            qualified &= passed;
            if passed {
                let b = fs::read(out.join(format!("{id}.device.bin")))?;
                let a: [u8; 8] = b
                    .try_into()
                    .map_err(|_| io::Error::other("invalid device identity length"))?;
                devices.push(u64::from_le_bytes(a));
            }
            let record = obj(&[
                ("case", quote(&id)),
                ("expected_exit_code", expected.to_string()),
                (
                    "actual_exit_code",
                    status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or("null".into()),
                ),
                ("timed_out", timed_out.to_string()),
                ("elapsed_ms", started.elapsed().as_millis().to_string()),
                ("qualified", passed.to_string()),
            ]);
            write_json(out.join(format!("{id}.process.json")), &record)?;
            results.push(record);
        }
    }
    let same_device =
        devices.len() == 8 && devices[0] != 0 && devices.iter().all(|d| *d == devices[0]);
    qualified &= same_device;
    write_json(out.join("summary.json"),&obj(&[("probe_version",quote(VERSION)),("backend",quote("native_metal_required")),
        ("gpu_probe_qualified",qualified.to_string()),("same_device_all_dispatches",same_device.to_string()),
        ("finite_input_count",corpus(false).len().to_string()),("nonfinite_input_count",corpus(true).len().to_string()),
        ("fast_math_settings","[false,true]".into()),("processes",array(results)),
        ("scope",quote("Qualification means both native math modes completed, input echoes match, known anchors passed, both wrong GPU variants failed anchors, and nonfinite observations completed. No blanket bit-identity or large-range accuracy claim."))]))?;
    Ok(if qualified { 0 } else { 2 })
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
                    || items.len() >= 128
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
        ("scope",quote("Integrity receipt only; consult native, process and summary receipts for device and command qualification. AIR is offline compiler evidence, not execution proof."))]))?;
    Ok(0)
}
fn entry() -> io::Result<i32> {
    let a: Vec<_> = env::args().collect();
    if a.len() == 7 && a[1] == "--child" {
        let parse = |s: &str| {
            s.parse::<u32>()
                .map_err(|_| io::Error::other("invalid child argument"))
        };
        if a[4] != "0" && a[4] != "1" {
            return Ok(64);
        }
        return child(
            Path::new(&a[5]),
            parse(&a[2])?,
            parse(&a[3])?,
            a[4] == "1",
            &a[6],
        );
    }
    if a.len() == 4 && a[1] == "seal" {
        return seal(
            Path::new(&a[2]),
            a[3].parse()
                .map_err(|_| io::Error::other("invalid exit code"))?,
        );
    }
    if a.len() != 3 {
        eprintln!("usage: unary-probe reference|reference-negative-swap|reference-negative-zero-cos|observe NEW_DIRECTORY; or seal DIRECTORY EXIT_CODE");
        return Ok(64);
    }
    match a[1].as_str() {
        "reference" => portable(Path::new(&a[2]), 0),
        "reference-negative-swap" => portable(Path::new(&a[2]), 1),
        "reference-negative-zero-cos" => portable(Path::new(&a[2]), 2),
        "observe" => observe(Path::new(&a[2])),
        _ => Ok(64),
    }
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
    fn hashes_known_vectors() {
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
    #[test]
    fn corpus_categories_and_separation() {
        let finite = corpus(false);
        let words = input_words(&finite);
        assert!(words.len() < 4096);
        assert!(words.iter().all(|&b| f32::from_bits(b).is_finite()));
        for b in [
            0, 0x80000000, 1, 0x80000001, 0x007fffff, 0x00800000, 0x80800000, 0x3f800000,
            0xbf800000, 0x7f7fffff, 0xff7fffff,
        ] {
            assert!(words.contains(&b));
        }
        for b in [
            0x3f490fdbu32,
            0x3fc90fdb,
            0x40490fdb,
            0x40c90fdb,
            0x43000000,
        ] {
            for n in [b - 1, b, b + 1] {
                assert!(words.contains(&n));
                assert!(words.contains(&(n | 0x80000000)));
            }
        }
        for e in 1..=254 {
            assert!(words.iter().any(|b| b >> 23 & 255 == e));
        }
        let nonfinite = corpus(true);
        assert_eq!(nonfinite.len(), 6);
        assert!(nonfinite
            .iter()
            .all(|c| !f32::from_bits(c.bits).is_finite()));
    }
    #[test]
    fn portable_reference_anchors() {
        let c = corpus(false);
        let k = check(&c, &reference(&c, 0));
        assert!(k.valid() && k.anchors_pass());
    }
    #[test]
    fn low_angle_grid_is_bounded_and_present() {
        let cases = corpus(false);
        let words = input_words(&cases);
        let upper = 3.0 * f32::from_bits(0x3ec90fdb) as f64;
        for i in 0..=128 {
            let bits = ((upper * i as f64 / 128.0) as f32).to_bits();
            assert!(words.contains(&bits));
            assert!(words.contains(&(bits | 0x80000000)));
        }
        assert!(cases.len() < 4096);
    }
    #[test]
    fn wrong_swap_must_fail_anchors() {
        let c = corpus(false);
        let k = check(&c, &reference(&c, 1));
        assert!(k.valid());
        assert!(!k.anchors_pass());
        assert!(k.failures.iter().any(|s| s.contains("cos_zero")));
    }
    #[test]
    fn wrong_zero_cos_must_fail_anchors() {
        let c = corpus(false);
        let k = check(&c, &reference(&c, 2));
        assert!(k.valid());
        assert!(!k.anchors_pass());
        assert!(k.failures.iter().any(|s| s.contains("cos_zero")));
    }
    #[test]
    fn corrupted_echo_and_length_fail() {
        let c = corpus(false);
        let mut r = reference(&c, 0);
        r[0] ^= 1;
        assert!(!check(&c, &r).valid());
        r.pop();
        assert!(!check(&c, &r).valid());
    }
    #[test]
    fn missing_anchors_and_nan_cannot_pass() {
        let c = corpus(true);
        assert!(!check(&c, &reference(&c, 0)).anchors_pass());
        let c = corpus(false);
        let mut r = reference(&c, 0);
        r[7] = 0x7fc00000;
        assert!(!check(&c, &r).anchors_pass());
    }
    #[test]
    fn evidence_fails_closed() {
        assert_eq!(std::mem::size_of::<Evidence>(), 32);
        assert!(!Evidence::default().valid(0));
        let mut e = Evidence {
            abi_version: 1,
            command_status: 4,
            completed: 1,
            no_error: 1,
            readback_copied: 1,
            fast_math_observed: 0,
            device_registry_id: 1,
        };
        assert!(e.valid(0));
        assert!(!e.valid(1));
        e.command_status = 5;
        assert!(!e.valid(0));
        e.command_status = 4;
        e.no_error = 0;
        assert!(!e.valid(0));
    }
    #[test]
    fn json_and_layout() {
        assert_eq!(quote("\"\\\n\0"), "\"\\\"\\\\\\u000a\\u0000\"");
        assert_eq!(bytes(&[0x12345678]), [0x78, 0x56, 0x34, 0x12]);
    }
}
