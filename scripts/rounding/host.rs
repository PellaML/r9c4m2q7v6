// Dependency-free Rust driver/reference models. No project code or captured data.
use std::{
    env, fs, io,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const CASES: &[(&str, [u32; 4])] = &[
    ("channel_order", [0x00000000, 0x3e800000, 0x3f000000, 0x3f400000]),
    ("clamp_and_signed_zero", [0x80000000, 0x3f800000, 0xbf800000, 0x40000000]),
    ("unorm_exact_tie_and_neighbors", [0x3effffff, 0x3f000000, 0x3f000001, 0x3efffffe]),
    ("half_even_tie_positive", [0x3f800fff, 0x3f801000, 0x3f801001, 0x3f800000]),
    ("half_even_tie_negative", [0xbf800fff, 0xbf801000, 0xbf801001, 0xbf800000]),
    ("half_odd_tie_positive", [0x3f802fff, 0x3f803000, 0x3f803001, 0x3f804000]),
    ("half_odd_tie_negative", [0xbf802fff, 0xbf803000, 0xbf803001, 0xbf804000]),
    ("half_zero_subnormal_tie_positive", [0x32ffffff, 0x33000000, 0x33000001, 0x33800000]),
    ("half_zero_subnormal_tie_negative", [0xb2ffffff, 0xb3000000, 0xb3000001, 0xb3800000]),
    ("half_odd_subnormal_tie_positive", [0x33bfffff, 0x33c00000, 0x33c00001, 0x34000000]),
    ("half_odd_subnormal_tie_negative", [0xb3bfffff, 0xb3c00000, 0xb3c00001, 0xb4000000]),
    ("half_subnormal_normal_boundary_positive", [0x387fdfff, 0x387fe000, 0x387fe001, 0x38800000]),
    ("half_subnormal_normal_boundary_negative", [0xb87fdfff, 0xb87fe000, 0xb87fe001, 0xb8800000]),
    ("binary32_tiny_positive", [0x00000001, 0x007fffff, 0x00800000, 0x00000000]),
    ("binary32_tiny_negative", [0x80000001, 0x807fffff, 0x80800000, 0x80000000]),
    ("half_overflow_positive", [0x477fdfff, 0x477fe000, 0x477ff000, 0x47800000]),
    ("half_overflow_negative", [0xc77fdfff, 0xc77fe000, 0xc77ff000, 0xc7800000]),
    ("finite_extremes", [0x7f7fffff, 0xff7fffff, 0x7f7ffffe, 0xff7ffffe]),
    // These are adjacent binary32 values near rational UNORM thresholds, NOT exact ties.
    ("unorm_near_low_thresholds", [0x3b008080, 0x3b008081, 0x3bc0c0c0, 0x3bc0c0c1]),
    ("unorm_near_middle_thresholds", [0x3e7efefe, 0x3e7efeff, 0x3e818181, 0x3e818182]),
    ("unorm_near_high_thresholds", [0x3efdfdfd, 0x3efdfdfe, 0x3f7f7f7e, 0x3f7f7f7f]),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Policy {
    Rne,
    Rtz,
}
#[derive(Clone, Copy, Debug)]
enum Format {
    Rgba8,
    Bgra8,
    Rgba16,
}
const FORMATS: [Format; 3] = [Format::Rgba8, Format::Bgra8, Format::Rgba16];
impl Format {
    fn name(self) -> &'static str {
        match self {
            Self::Rgba8 => "rgba8",
            Self::Bgra8 => "bgra8",
            Self::Rgba16 => "rgba16f",
        }
    }
    fn bpp(self) -> usize {
        if matches!(self, Self::Rgba16) {
            8
        } else {
            4
        }
    }
}
#[derive(Clone, Copy)]
struct Model {
    name: &'static str,
    policy: Policy,
    fp32_scale: bool,
}
fn models(format: Format) -> Vec<Model> {
    let mut out = vec![
        Model { name: "rne_exact", policy: Policy::Rne, fp32_scale: false },
        Model { name: "rtz_exact", policy: Policy::Rtz, fp32_scale: false },
    ];
    if !matches!(format, Format::Rgba16) {
        out.extend([
            Model { name: "rne_fp32_scale", policy: Policy::Rne, fp32_scale: true },
            Model { name: "rtz_fp32_scale", policy: Policy::Rtz, fp32_scale: true },
        ]);
    }
    out
}
fn shifted(n: u64, shift: u32, policy: Policy) -> u64 {
    // All callers have n < 2^32, so shifts >= 64 cannot reach a half-way point.
    if shift >= 64 {
        return 0;
    }
    if shift == 0 {
        return n;
    }
    let q = n >> shift;
    let r = n & ((1u64 << shift) - 1);
    let half = 1u64 << (shift - 1);
    q + u64::from(policy == Policy::Rne && (r > half || (r == half && q & 1 != 0)))
}
fn half(bits: u32, policy: Policy) -> u16 {
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = (bits >> 23) & 255;
    let fraction = bits & 0x7fffff;
    assert!(exponent != 255 || fraction == 0, "NaNs intentionally outside this fixture contract");
    if exponent == 255 {
        return sign | 0x7c00;
    }
    let mantissa = fraction | if exponent == 0 { 0 } else { 0x800000 };
    if exponent < 113 {
        return sign | shifted(mantissa as u64, 126 - exponent.max(1), policy) as u16;
    }
    if exponent >= 143 {
        return sign | if policy == Policy::Rne { 0x7c00 } else { 0x7bff };
    }
    let magnitude = ((exponent - 112) << 10) + shifted(mantissa as u64, 13, policy) as u32 - 1024;
    sign | magnitude as u16
}
fn unorm(bits: u32, model: Model) -> u8 {
    assert!((bits & 0x7fffffff) <= 0x7f800000, "NaNs excluded");
    if bits >> 31 != 0 {
        return 0;
    }
    if bits >= 0x3f800000 {
        return 255;
    }
    if model.fp32_scale {
        // Diagnostic candidate for an intermediate, binary32-rounded x*255.
        // It is NOT silently substituted for the exact rational model.
        let x = f32::from_bits(bits) * 255.0;
        return if model.policy == Policy::Rne { x.round_ties_even() as u8 } else { x as u8 };
    }
    let exponent = bits >> 23;
    let mantissa = (bits & 0x7fffff) | if exponent == 0 { 0 } else { 0x800000 };
    shifted(mantissa as u64 * 255, 150 - exponent.max(1), model.policy) as u8
}
fn input_words() -> Vec<u32> {
    CASES.iter().flat_map(|(_, v)| *v).collect()
}
fn words_le(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn expected(format: Format, model: Model) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (_, values) in CASES {
        if matches!(format, Format::Rgba16) {
            for b in values {
                bytes.extend(half(*b, model.policy).to_le_bytes());
            }
        } else {
            let order = if matches!(format, Format::Bgra8) { [2, 1, 0, 3] } else { [0, 1, 2, 3] };
            for c in order {
                bytes.push(unorm(values[c], model));
            }
        }
    }
    bytes
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
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
fn array(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(","))
}
fn object(items: &[(&str, String)]) -> String {
    format!("{{{}}}", items.iter().map(|(k, v)| format!("{}:{v}", quote(k))).collect::<Vec<_>>().join(","))
}
fn write_json(path: impl AsRef<Path>, text: &str) -> io::Result<()> {
    fs::write(path, format!("{text}\n"))
}
fn mismatch(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len())
}
fn diff(a: &[u8], b: &[u8]) -> String {
    object(&[
        ("mismatched_bytes", mismatch(a, b).to_string()),
        (
            "first_mismatches",
            array(a.iter().zip(b).enumerate().filter(|(_, (x, y))| x != y).take(8).map(|(i, (x, y))| {
                object(&[("byte", i.to_string()), ("actual", x.to_string()), ("expected", y.to_string())])
            })),
        ),
    ])
}
fn expected_name(format: Format, model: Model) -> String {
    format!("expected-{}-{}.bin", format.name(), model.name)
}
fn fixtures(out: &Path) -> io::Result<()> {
    fs::write(out.join("inputs.bin"), words_le(&input_words()))?;
    let mut refs = Vec::new();
    for format in FORMATS {
        for model in models(format) {
            let data = expected(format, model);
            let name = expected_name(format, model);
            fs::write(out.join(&name), &data)?;
            refs.push(object(&[("file", quote(&name)), ("hex", quote(&hex(&data)))]));
        }
    }
    let rows = CASES.iter().enumerate().map(|(i, (name, bits))| {
        object(&[
            ("pixel", i.to_string()),
            ("case", quote(name)),
            ("rgba_binary32_hex", array(bits.iter().map(|b| quote(&format!("{b:08x}"))))),
        ])
    });
    write_json(out.join("fixtures.json"), &object(&[
        ("schema", "1".into()), ("byte_order", quote("little-endian; tightly packed rows")),
        ("pixel_count", CASES.len().to_string()), ("cases", array(rows)), ("expected", array(refs)),
        ("exact_model", quote("exact input value; UNORM clamp then exact x*255; binary16 gradual underflow; RNE ties-even or RTZ")),
        ("fp32_scale_model", quote("UNORM only: binary32-rounded x*255, then RNE or RTZ; diagnostic, not a presumed Metal contract")),
        ("scope", quote("all inputs finite; includes signed zero, binary32 tiny inputs, half subnormals and finite overflow; no NaN/Infinity input assumptions under fast math")),
    ]))?;
    let controls = FORMATS.iter().map(|&f| {
        let m = models(f);
        let a = expected(f, m[0]);
        let b = expected(f, m[1]);
        assert!(mismatch(&a, &b) > 0);
        object(&[
            ("format", quote(f.name())),
            ("rne_vs_wrong_rtz", diff(&a, &b)),
            ("cpu_only_not_gpu_proof", "true".into()),
        ])
    });
    write_json(out.join("reference-controls.json"), &array(controls))
}

#[cfg(all(rounding_native, not(target_os = "macos")))]
compile_error!("rounding_native requires macOS");
#[cfg(rounding_native)]
extern "C" {
    fn rounding_native(
        stage: u32,
        mode: u32,
        format: u32,
        fast_math: u32,
        words: *const u32,
        pixels: usize,
        source: *const std::ffi::c_char,
        raw: *mut u8,
        raw_size: usize,
        echo: *mut u32,
        observed_device: *mut u64,
        json: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
}
fn child(stage: u32, mode: u32, fmt: usize, fast_math: u32, out: &Path, id: &str) -> io::Result<i32> {
    if mode > 2 || fmt > 2 || stage > 3 || fast_math > 1 {
        return Ok(64);
    }
    #[cfg(rounding_native)]
    {
        let words = input_words();
        let mut raw = vec![0x5a; CASES.len() * FORMATS[fmt].bpp()];
        let mut echo = vec![0xcccccccc; words.len()];
        let mut observed_device = 0u64;
        let mut json = vec![0u8; 65536];
        let source = std::ffi::CString::new(include_str!("write.metal")).unwrap();
        let status = unsafe {
            rounding_native(
                stage,
                mode,
                fmt as u32,
                fast_math,
                words.as_ptr(),
                CASES.len(),
                source.as_ptr(),
                raw.as_mut_ptr(),
                raw.len(),
                echo.as_mut_ptr(),
                &mut observed_device,
                json.as_mut_ptr().cast(),
                json.len(),
            )
        };
        let size = json.iter().position(|&b| b == 0).unwrap_or(json.len());
        if size == 0 {
            return Ok(70);
        }
        fs::write(out.join(format!("{id}.native.json")), &json[..size])?;
        if status == 0 && stage == 1 {
            fs::write(out.join(format!("{id}.raw.bin")), raw)?;
            fs::write(out.join(format!("{id}.echo.bin")), words_le(&echo))?;
            fs::write(out.join(format!("{id}.device.bin")), observed_device.to_le_bytes())?;
        }
        Ok(status)
    }
    #[cfg(not(rounding_native))]
    {
        write_json(
            out.join(format!("{id}.native.json")),
            &object(&[
                ("status", quote("platform_or_native_bridge_unavailable")),
                ("os", quote(env::consts::OS)),
                ("arch", quote(env::consts::ARCH)),
                ("stage", stage.to_string()),
                ("requested_mode", mode.to_string()),
                ("fast_math_requested", (fast_math != 0).to_string()),
                ("command_completed", "false".into()),
                ("raw_readback_copied", "false".into()),
            ]),
        )?;
        Ok(30)
    }
}
fn launch(stage: u32, mode: u32, fmt: usize, fast_math: u32, out: &Path, id: &str) -> io::Result<bool> {
    let log = fs::File::create(out.join(format!("{id}.stderr.log")))?;
    let mut child = Command::new(env::current_exe()?)
        .args(["--child", &stage.to_string(), &mode.to_string(), &fmt.to_string(), &fast_math.to_string()])
        .arg(out)
        .arg(id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()?;
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(if stage == 1 { 30 } else { 15 }) {
            timed_out = true;
            child.kill()?;
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(50));
    };
    write_json(
        out.join(format!("{id}.process.json")),
        &object(&[
            ("exit_code", status.code().map_or("null".into(), |n| n.to_string())),
            ("timed_out", timed_out.to_string()),
            ("success", (status.success() && !timed_out).to_string()),
            ("signal_or_abnormal_exit", status.code().is_none().to_string()),
            (
                "note",
                quote("nonzero, signal or timeout is not an observed rounding policy; stderr is local-only"),
            ),
        ]),
    )?;
    Ok(status.success() && !timed_out)
}
fn classify(out: &Path, id: &str, format: Format) -> io::Result<(bool, bool)> {
    let raw = fs::read(out.join(format!("{id}.raw.bin")))?;
    let echo = fs::read(out.join(format!("{id}.echo.bin")))?;
    let length_ok = raw.len() == CASES.len() * format.bpp();
    let echo_ok = echo == words_le(&input_words());
    let nonpoison = raw.iter().any(|&b| b != 0xa5) && raw.iter().any(|&b| b != 0x5a);
    let valid = length_ok && echo_ok && nonpoison;
    let candidates = models(format);
    let matched: Vec<_> = candidates.iter().filter(|m| valid && raw == expected(format, **m)).collect();
    let unique_policy = !matched.is_empty() && matched.iter().all(|m| m.policy == matched[0].policy);
    let checks = candidates
        .iter()
        .map(|&m| object(&[("candidate", quote(m.name)), ("difference", diff(&raw, &expected(format, m)))]));
    let mut control = object(&[("status", quote("not_run_no_unique_candidate"))]);
    let mut control_ok = false;
    if unique_policy {
        let model = *matched[0];
        let other = *candidates
            .iter()
            .find(|m| m.policy != model.policy && m.fp32_scale == model.fp32_scale)
            .unwrap();
        let correct = expected(format, model);
        let wrong = expected(format, other);
        if let Some(witness) = correct.iter().zip(&wrong).position(|(a, b)| a != b) {
            let mut one_wrong = correct.clone();
            one_wrong[witness] = wrong[witness];
            let wrong_name = format!("{id}.wrong-expected.bin");
            fs::write(out.join(&wrong_name), &one_wrong)?;
            control_ok = mismatch(&raw, &wrong) > 0 && mismatch(&raw, &one_wrong) == 1;
            control = object(&[
                ("status", quote(if control_ok { "rejected_wrong_rounding" } else { "control_failed" })),
                ("opposite_policy_file", quote(&expected_name(format, other))),
                ("opposite_policy_difference", diff(&raw, &wrong)),
                ("single_wrong_byte_file", quote(&wrong_name)),
                ("witness_byte", witness.to_string()),
                ("single_wrong_byte_difference", diff(&raw, &one_wrong)),
            ]);
        }
    }
    // When no whole-vector model matches, retain per-pixel diagnostics. In particular,
    // subnormal failures remain visible, never excused by dropping those cases.
    let row_diffs = CASES.iter().enumerate().map(|(pixel, (name, _))| {
        let start = pixel * format.bpp();
        let end = start + format.bpp();
        let actual = raw.get(start..end).unwrap_or(&[]);
        object(&[
            ("pixel", pixel.to_string()),
            ("case", quote(name)),
            ("actual_hex", quote(&hex(actual))),
            (
                "matching_candidates",
                array(
                    candidates
                        .iter()
                        .filter(|&&m| actual == &expected(format, m)[start..end])
                        .map(|m| quote(m.name)),
                ),
            ),
        ])
    });
    write_json(out.join(format!("{id}.check.json")), &object(&[
        ("raw_length_ok", length_ok.to_string()), ("gpu_input_echo_exact", echo_ok.to_string()),
        ("not_uniform_poison", nonpoison.to_string()), ("valid_readback", valid.to_string()),
        ("matching_models", array(matched.iter().map(|m| quote(m.name)))),
        ("unique_candidate_policy", unique_policy.to_string()), ("candidate_differences", array(checks)),
        ("pixels", array(row_diffs)), ("negative_control", control),
        ("candidate_policy", if unique_policy { quote(if matched[0].policy == Policy::Rne { "RNE" } else { "RTZ" }) } else { "null".into() }),
        ("scope", quote("candidate fit for this source/input/runtime/device/format only; not a global enum or wire mapping")),
    ]))?;
    Ok((valid, valid && unique_policy && control_ok))
}
fn run(kind: &str, out: &Path) -> io::Result<i32> {
    // Refuse reuse to prevent stale native files from being mistaken for a new run.
    fs::create_dir(out)?;
    fixtures(out)?;
    if kind == "fixtures" {
        write_json(
            out.join("summary.json"),
            &object(&[
                ("kind", quote(kind)),
                ("shader_oracle_passed", "false".into()),
                ("reason", quote("CPU reference generation only")),
            ]),
        )?;
        return Ok(0);
    }
    let inventory_ok = launch(2, 0, 0, 0, out, "device")?;
    let mut descriptors = 0;
    for mode in 0..3 {
        if launch(0, mode, 0, 0, out, &format!("mode-{mode}-descriptor"))? {
            descriptors += 1;
        }
        let _ = launch(3, mode, 0, 0, out, &format!("mode-{mode}-capability"))?;
    }
    let mut readbacks = 0;
    let mut qualified = 0;
    let mut valid_cases = [[[false; 3]; 3]; 2];
    let mut classified_cases = [[[false; 3]; 3]; 2];
    let mut device_ids = [[[0u64; 3]; 3]; 2];
    if kind == "observe" {
        for fast in 0..2 {
            for mode in 0..3 {
                for (fmt, format) in FORMATS.iter().enumerate() {
                    let id = format!("fast-{fast}-mode-{mode}-{}", format.name());
                    if launch(1, mode as u32, fmt, fast as u32, out, &id)? {
                        let (valid, qualifies) = classify(out, &id, *format)?;
                        readbacks += usize::from(valid);
                        qualified += usize::from(qualifies);
                        valid_cases[fast][mode][fmt] = valid;
                        classified_cases[fast][mode][fmt] = qualifies;
                        let device = fs::read(out.join(format!("{id}.device.bin")))?;
                        device_ids[fast][mode][fmt] = u64::from_le_bytes(
                            device
                                .try_into()
                                .map_err(|_| io::Error::other("invalid device identity byte length"))?,
                        );
                    } else {
                        write_json(
                            out.join(format!("{id}.check.json")),
                            &object(&[
                                ("valid_readback", "false".into()),
                                ("status", quote("unavailable_or_failed_see_native_and_process_reports")),
                            ]),
                        )?;
                    }
                }
            }
        }
    }
    let mut comparisons = Vec::new();
    for fast in 0..2 {
        for (fmt, format) in FORMATS.iter().enumerate() {
            let mut pairs = Vec::new();
            let all_observed = (0..3).all(|mode| valid_cases[fast][mode][fmt]);
            let all_classified = (0..3).all(|mode| classified_cases[fast][mode][fmt]);
            let same_device = all_observed
                && device_ids[fast][0][fmt] != 0
                && (1..3).all(|mode| device_ids[fast][mode][fmt] == device_ids[fast][0][fmt]);
            let mut all_equal = all_observed;
            for (a, b) in [(0, 1), (0, 2), (1, 2)] {
                let observed = valid_cases[fast][a][fmt] && valid_cases[fast][b][fmt];
                let comparison = if observed {
                    let left = fs::read(out.join(format!("fast-{fast}-mode-{a}-{}.raw.bin", format.name())))?;
                    let right =
                        fs::read(out.join(format!("fast-{fast}-mode-{b}-{}.raw.bin", format.name())))?;
                    all_equal &= left == right;
                    diff(&left, &right)
                } else {
                    "null".into()
                };
                pairs.push(object(&[
                    ("mode_a", a.to_string()),
                    ("mode_b", b.to_string()),
                    ("both_readbacks_valid", observed.to_string()),
                    (
                        "same_nonzero_device_id",
                        (observed
                            && device_ids[fast][a][fmt] != 0
                            && device_ids[fast][a][fmt] == device_ids[fast][b][fmt])
                            .to_string(),
                    ),
                    ("difference", comparison),
                ]));
            }
            comparisons.push(object(&[("format", quote(format.name())), ("fast_math", (fast != 0).to_string()),
            ("all_three_observed", all_observed.to_string()),
            ("same_device_for_all_three", same_device.to_string()),
            ("all_three_raw_outputs_identical", if all_observed { all_equal.to_string() } else { "null".into() }),
            ("all_three_candidate_and_control_checks_pass", all_classified.to_string()),
            ("pairs", array(pairs)),
            ("scope", quote("compute writes for this fixture only; byte equality alone does not prove correctness or universal property inapplicability"))]));
        }
    }
    write_json(out.join("cross-mode-comparisons.json"), &array(comparisons))?;
    let mut math_comparisons = Vec::new();
    for mode in 0..3 {
        for (fmt, format) in FORMATS.iter().enumerate() {
            let observed = valid_cases[0][mode][fmt] && valid_cases[1][mode][fmt];
            let same_device = observed
                && device_ids[0][mode][fmt] != 0
                && device_ids[0][mode][fmt] == device_ids[1][mode][fmt];
            let comparison = if observed {
                let slow = fs::read(out.join(format!("fast-0-mode-{mode}-{}.raw.bin", format.name())))?;
                let fast = fs::read(out.join(format!("fast-1-mode-{mode}-{}.raw.bin", format.name())))?;
                diff(&slow, &fast)
            } else {
                "null".into()
            };
            math_comparisons.push(object(&[
                ("mode", mode.to_string()),
                ("format", quote(format.name())),
                ("both_math_settings_observed", observed.to_string()),
                ("same_device", same_device.to_string()),
                (
                    "both_candidate_and_control_checks_pass",
                    (classified_cases[0][mode][fmt] && classified_cases[1][mode][fmt]).to_string(),
                ),
                ("false_versus_true_difference", comparison),
            ]));
        }
    }
    write_json(out.join("fast-math-comparisons.json"), &array(math_comparisons))?;
    let same_device = readbacks == 18
        && device_ids[0][0][0] != 0
        && device_ids.iter().flatten().flatten().all(|&id| id == device_ids[0][0][0]);
    let passed = kind == "observe" && descriptors == 3 && readbacks == 18 && qualified == 18 && same_device;
    write_json(out.join("summary.json"), &object(&[
        ("schema", "1".into()), ("kind", quote(kind)), ("inventory_process_ok", inventory_ok.to_string()),
        ("descriptor_round_trips_observed", descriptors.to_string()),
        ("fast_math_settings", "[false,true]".into()),
        ("gpu_cases_requested", if kind == "observe" { "18" } else { "0" }.into()),
        ("valid_gpu_readbacks", readbacks.to_string()), ("uniquely_classified_with_negative_control", qualified.to_string()),
        ("same_device_for_all_gpu_cases", same_device.to_string()),
        ("shader_oracle_passed", passed.to_string()),
        ("global_enum_mapping_established", "false".into()), ("external_wire_mapping_established", "false".into()),
        ("scope", quote("current-runtime hardware observation only; description names are framework text, not guessed enum definitions")),
    ]))?;
    println!("descriptor round trips: {descriptors}/3; GPU readbacks: {readbacks}/18; classified/control-checked: {qualified}/18; shader_oracle_passed={passed}");
    Ok(if passed || (kind == "inspect" && descriptors == 3 && inventory_ok) { 0 } else { 2 })
}
fn entry() -> io::Result<i32> {
    let args: Vec<_> = env::args().collect();
    if args.len() == 8 && args[1] == "--child" {
        let parse = |s: &str| s.parse().map_err(|_| io::Error::other("invalid child argument"));
        return child(
            parse(&args[2])?,
            parse(&args[3])?,
            parse(&args[4])? as usize,
            parse(&args[5])?,
            Path::new(&args[6]),
            &args[7],
        );
    }
    if args.len() != 3 || !["fixtures", "inspect", "observe"].contains(&args[1].as_str()) {
        eprintln!("usage: rounding-probe fixtures|inspect|observe NEW_OUTPUT_DIRECTORY");
        return Ok(64);
    }
    run(&args[1], Path::new(&args[2]))
}
fn main() {
    std::process::exit(match entry() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("probe I/O error: {e}");
            1
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn exact(policy: Policy) -> Model {
        Model { name: "test", policy, fp32_scale: false }
    }
    fn widen(h: u16) -> f64 {
        let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let e = ((h >> 10) & 31) as i32;
        let f = (h & 1023) as f64;
        sign * if e == 0 { f * 2f64.powi(-24) } else { (1024.0 + f) * 2f64.powi(e - 25) }
    }
    #[test]
    fn exhaustive_finite_half_midpoints_and_neighbors_both_signs() {
        for h in 0..0x7bffu16 {
            let mid = ((widen(h) + widen(h + 1)) * 0.5) as f32;
            let tie = if h & 1 == 0 { h } else { h + 1 };
            for sign in [0u32, 0x80000000] {
                let hs = (sign >> 16) as u16;
                let b = mid.to_bits();
                assert_eq!(half(b | sign, Policy::Rne), tie | hs);
                assert_eq!(half((b - 1) | sign, Policy::Rne), h | hs);
                assert_eq!(half((b + 1) | sign, Policy::Rne), (h + 1) | hs);
                for input in [b - 1, b, b + 1] {
                    assert_eq!(half(input | sign, Policy::Rtz), h | hs);
                }
            }
        }
    }
    #[test]
    fn exact_half_identity_and_edges() {
        for h in 0..0x7c00u16 {
            for sign in [0u16, 0x8000] {
                let bits = (widen(h | sign) as f32).to_bits();
                for p in [Policy::Rne, Policy::Rtz] {
                    assert_eq!(half(bits, p), h | sign);
                }
            }
        }
        assert_eq!(half(0x477ff000, Policy::Rne), 0x7c00);
        assert_eq!(half(0x477ff000, Policy::Rtz), 0x7bff);
        assert_eq!(half(0xc77ff000, Policy::Rne), 0xfc00);
        assert_eq!(half(0xc77ff000, Policy::Rtz), 0xfbff);
        assert_eq!(half(0x33000000, Policy::Rne), 0);
        assert_eq!(half(0x33000001, Policy::Rne), 1);
    }
    #[test]
    fn unorm_tie_clamp_and_scale_are_explicit() {
        assert_eq!(unorm(0x3f000000, exact(Policy::Rne)), 128);
        assert_eq!(unorm(0x3f000000, exact(Policy::Rtz)), 127);
        assert_eq!(unorm(0x3effffff, exact(Policy::Rne)), 127);
        assert_eq!(unorm(0x3f000001, exact(Policy::Rne)), 128);
        for p in [Policy::Rne, Policy::Rtz] {
            assert_eq!(unorm(0xbf800000, exact(p)), 0);
            assert_eq!(unorm(0x40000000, exact(p)), 255);
            assert_eq!(unorm(1, exact(p)), 0);
        }
        // Float32 scaling can land on a tie even when the true scaled input is above it.
        assert_eq!(unorm(0x3b008081, exact(Policy::Rne)), 1);
        assert_eq!(unorm(0x3b008081, Model { fp32_scale: true, ..exact(Policy::Rne) }), 0);
    }
    #[test]
    fn unorm_exact_matches_independent_binary64_reference() {
        // A binary32 mantissa times 255 is exactly representable in binary64.
        let mut state = 0x12345678u32;
        for _ in 0..100000 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let bits = state % 0x7f800001;
            let x = (f32::from_bits(bits) as f64).clamp(0.0, 1.0) * 255.0;
            assert_eq!(unorm(bits, exact(Policy::Rne)), x.round_ties_even() as u8);
            assert_eq!(unorm(bits, exact(Policy::Rtz)), x as u8);
        }
    }
    #[test]
    fn byte_order_swizzle_and_wrong_rounding_controls() {
        assert!(input_words().iter().all(|&b| f32::from_bits(b).is_finite()));
        assert_eq!(&expected(Format::Rgba8, exact(Policy::Rne))[..4], &[0, 64, 128, 191]);
        assert_eq!(&expected(Format::Bgra8, exact(Policy::Rne))[..4], &[128, 64, 0, 191]);
        assert_eq!(&expected(Format::Rgba16, exact(Policy::Rne))[..8], &[0, 0, 0, 0x34, 0, 0x38, 0, 0x3a]);
        for f in FORMATS {
            let a = expected(f, exact(Policy::Rne));
            let b = expected(f, exact(Policy::Rtz));
            assert!(mismatch(&a, &b) > 0);
            let i = a.iter().zip(&b).position(|(x, y)| x != y).unwrap();
            let mut wrong = a.clone();
            wrong[i] = b[i];
            assert_eq!(mismatch(&a, &wrong), 1);
        }
    }
    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(quote("\"\\\n\0"), "\"\\\"\\\\\\u000a\\u0000\"");
    }
}
