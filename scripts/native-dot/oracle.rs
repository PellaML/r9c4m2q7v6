// Standalone, std-only validation I/O. No host floating-point oracle or model.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

const SOURCE: &[u8] = include_bytes!("probe.metal");
const POISON: u32 = 0xa5a5_a5a5;
const GUARD: usize = 64;
const WORDS: usize = 32;
const PROFILES: [&str; 3] = ["false", "true", "default"];
const KERNELS: [&str; 9] = [
    "dot2",
    "dot3",
    "dot4",
    "add2",
    "add3",
    "add4",
    "noop",
    "compose_trace",
    "compose_sink",
];
const COMMAND_HEADER: &str = "kernel\trepeat\tstatus\tno_error\twait_called\tinputs_unchanged\tbytes\tthread_width\tmax_threads\tgpu_start\tgpu_end";
type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Case {
    name: &'static str,
    group: &'static str,
    a: [u32; 4],
    b: [u32; 4],
}

fn cases() -> Vec<Case> {
    // Deliberately small, generic public IEEE patterns; no captures or shaders.
    // a,b feed direct dot. compose_* instead normalizes a.xy and projects b.xy.
    let z = 0;
    let nz = 0x8000_0000;
    let one = 0x3f80_0000;
    let q = 0x7fc0_0000;
    let inf = 0x7f80_0000;
    let max = 0x7f7f_ffff;
    [
        (
            "exact_anchor",
            "finite_anchor",
            [one, 0x40000000, 0x40400000, 0x40800000],
            [0x40a00000, 0x40c00000, 0x40e00000, 0x41000000],
        ),
        (
            "mixed_sign",
            "finite",
            [0x40000000, 0xc0400000, 0x40800000, 0xc0a00000],
            [0x40e00000, 0x41300000, 0xc1500000, 0x41880000],
        ),
        (
            "signed_zero_qnan2",
            "nan_input",
            [nz, z, z, z],
            [q, q, z, z],
        ),
        (
            "qnan_payload_sign",
            "nan_input",
            [0x7fc00042, 0xffc12345, z, z],
            [one, one, z, z],
        ),
        (
            "snan_payload_sign",
            "nan_input",
            [0x7f800001, 0xffa12345, z, z],
            [one, one, z, z],
        ),
        (
            "zero_vector_zero_delta",
            "zero_composition",
            [z, z, z, z],
            [nz, z, z, z],
        ),
        (
            "signed_zero_vector",
            "zero_composition",
            [z, nz, z, z],
            [z, nz, z, z],
        ),
        (
            "zero_vector_finite_delta",
            "zero_composition",
            [z, z, z, z],
            [0x3e800000, 0xbf000000, z, z],
        ),
        (
            "finite_normalization",
            "finite_composition",
            [0x40400000, 0x40800000, z, z],
            [0x3e800000, 0x3f000000, z, z],
        ),
        (
            "subnormal_vector",
            "underflow_composition",
            [0x00000001, 0x80000001, z, z],
            [0x3e800000, 0xbf000000, z, z],
        ),
        (
            "min_normal_vector",
            "underflow_composition",
            [0x00800000, z, z, z],
            [0x3e800000, 0xbf000000, z, z],
        ),
        (
            "small_normal_vector",
            "underflow_composition",
            [0x1e3ce508, z, z, z],
            [0x3e800000, 0xbf000000, z, z],
        ),
        (
            "large_vector",
            "finite_overflow",
            [max, max, z, z],
            [0x3e800000, 0xbf000000, z, z],
        ),
        (
            "infinity_positive",
            "infinity_input",
            [inf, one, z, z],
            [one, one, z, z],
        ),
        (
            "infinity_cancel",
            "infinity_input",
            [inf, 0xff800000, z, z],
            [one, one, z, z],
        ),
        (
            "zero_times_infinity",
            "infinity_input",
            [z, nz, z, z],
            [inf, inf, z, z],
        ),
        (
            "finite_product_overflow",
            "finite_overflow",
            [max, max, z, z],
            [0x40000000, 0xc0000000, z, z],
        ),
        (
            "finite_sum_overflow_order",
            "finite_overflow",
            [max, max, 0xff7fffff, z],
            [one, one, one, z],
        ),
        (
            "finite_cancellation_order",
            "rounding_order",
            [0x4b800000, one, 0xcb800000, z],
            [one, one, one, z],
        ),
        (
            "finite_contraction_witness",
            "rounding_order",
            [0x3f800001, 0xbf800000, z, z],
            [0x3f7ffffe, one, z, z],
        ),
        (
            "signed_zero_products",
            "zero_input",
            [nz, nz, z, z],
            [one, one, one, one],
        ),
        (
            "inactive_tail_poison2",
            "width_control",
            [one, 0x40000000, 0x7fc00042, inf],
            [0x40400000, 0x40800000, q, z],
        ),
        (
            "inactive_tail_poison3",
            "width_control",
            [one, 0x40000000, 0x40400000, q],
            [0x40a00000, 0x40c00000, 0x40e00000, q],
        ),
        (
            "subnormal_times_infinity",
            "infinity_input",
            [0x00000001, z, z, z],
            [inf, z, z, z],
        ),
        (
            "unit_vector_inside",
            "composition_control",
            [one, z, z, z],
            [z, 0x3f000000, z, z],
        ),
        (
            "unit_vector_outside",
            "composition_control",
            [one, z, z, z],
            [z, 0x40000000, z, z],
        ),
    ]
    .into_iter()
    .map(|(name, group, a, b)| Case { name, group, a, b })
    .collect()
}

fn input_bytes(cases: &[Case]) -> Vec<u8> {
    cases
        .iter()
        .flat_map(|c| c.a.into_iter().chain(c.b))
        .flat_map(u32::to_le_bytes)
        .collect()
}

fn nan(bits: u32) -> bool {
    bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0
}
fn finite(bits: u32) -> bool {
    bits & 0x7f80_0000 != 0x7f80_0000
}
fn class(bits: u32) -> String {
    let sign = if bits >> 31 == 0 { "+" } else { "-" };
    let exponent = bits & 0x7f80_0000;
    let fraction = bits & 0x007f_ffff;
    let name = match (exponent, fraction) {
        (0, 0) => "zero",
        (0, _) => "subnormal",
        (0x7f80_0000, 0) => "infinity",
        (0x7f80_0000, _) if fraction & 0x0040_0000 != 0 => "qnan",
        (0x7f80_0000, _) => "snan",
        _ => "normal",
    };
    if nan(bits) {
        format!("{sign}{name}:fraction={fraction:06x}")
    } else {
        format!("{sign}{name}")
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| e.to_string())
}
fn read(path: &Path, max: u64) -> Result<Vec<u8>> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !m.is_file() || m.len() > max {
        return Err(format!("not a bounded regular file: {}", path.display()));
    }
    fs::read(path).map_err(|e| e.to_string())
}
fn text(path: &Path, max: u64) -> Result<String> {
    String::from_utf8(read(path, max)?).map_err(|_| "non-UTF8 record".into())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn prepare(root: &Path) -> Result<()> {
    let c = cases();
    require(c.len() == 26, "cohort count drift")?;
    let mut rows = String::from("row\tlabel\tgroup\tside\tlane\tbits\tclass\n");
    for (row, c) in c.iter().enumerate() {
        for (side, words) in [("a", c.a), ("b", c.b)] {
            for (lane, bits) in words.into_iter().enumerate() {
                writeln!(
                    rows,
                    "{row}\t{}\t{}\t{side}\t{lane}\t{bits:08x}\t{}",
                    c.name,
                    c.group,
                    class(bits)
                )
                .unwrap();
            }
        }
    }
    write_new(&root.join("inputs.bin"), &input_bytes(&c))?;
    write_new(&root.join("cases.tsv"), rows.as_bytes())
}

fn unescape(value: &str) -> Result<String> {
    let mut out = String::new();
    let mut it = value.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match it.next() {
            Some('\\') => '\\',
            Some('t') => '\t',
            Some('r') => '\r',
            Some('n') => '\n',
            _ => return Err("invalid TSV escape".into()),
        });
    }
    Ok(out)
}
fn metadata(s: &str) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for line in s.lines() {
        let (key, value) = line.split_once('\t').ok_or("malformed metadata")?;
        require(
            !key.is_empty() && !value.contains('\t'),
            "malformed metadata fields",
        )?;
        require(
            fields.insert(key.to_owned(), unescape(value)?).is_none(),
            "duplicate metadata key",
        )?;
    }
    Ok(fields)
}
fn get<'a>(m: &'a BTreeMap<String, String>, k: &str) -> Result<&'a str> {
    m.get(k)
        .map(String::as_str)
        .ok_or_else(|| format!("missing metadata: {k}"))
}
fn check_meta(
    m: &BTreeMap<String, String>,
    profile: &str,
    build: u32,
    count: usize,
) -> Result<String> {
    for (key, expected) in [
        ("schema", "native-dot-v1"),
        ("backend", "native_metal"),
        ("cpu_fallback", "0"),
        ("selection", "system_default_only"),
        ("compiler_route", "newLibraryWithSource"),
        ("other_math_setters_called", "0"),
        ("buffer_storage", "shared"),
        ("guard_bytes_each_side", "64"),
        ("row_words", "32"),
        ("completed_commands", "27"),
        ("status", "native_bytes_observed_not_model_qualification"),
        ("host_pointer_bits", "64"),
    ] {
        require(
            get(m, key)? == expected,
            &format!("metadata mismatch: {key}"),
        )?;
    }
    require(
        !m.contains_key("failure_phase") && !m.contains_key("exception_name"),
        "failed native receipt",
    )?;
    require(
        get(m, "profile")? == profile && get(m, "build_request")? == build.to_string(),
        "profile/build binding",
    )?;
    require(
        get(m, "case_count")? == count.to_string()
            && get(m, "source_bytes")? == SOURCE.len().to_string(),
        "input/source size binding",
    )?;
    let before = get(m, "fast_math_before")?;
    let after = get(m, "fast_math_after")?;
    require(
        ["0", "1"].contains(&before) && ["0", "1"].contains(&after),
        "invalid math getter",
    )?;
    if profile == "default" {
        require(
            get(m, "fast_math_setter_called")? == "0" && before == after,
            "default must be untouched",
        )?;
    } else {
        require(
            get(m, "fast_math_setter_called")? == "1"
                && after == if profile == "true" { "1" } else { "0" },
            "explicit math profile mismatch",
        )?;
    }
    for key in ["unified_memory", "low_power", "removable", "headless"] {
        require(["0", "1"].contains(&get(m, key)?), "invalid device Boolean")?;
    }
    for key in [
        "language_version_getter",
        "library_type",
        "max_threadgroup_memory",
        "max_buffer_length",
        "max_threads_x",
        "max_threads_y",
        "max_threads_z",
    ] {
        let _: u64 = get(m, key)?
            .parse()
            .map_err(|_| format!("invalid number: {key}"))?;
    }
    for key in ["math_mode_getter", "math_functions_getter"] {
        let value = get(m, key)?;
        require(
            value == "unavailable" || value.parse::<u64>().is_ok(),
            "invalid modern option getter",
        )?;
    }
    for key in [
        "device_name",
        "os_version_and_build",
        "metal_bundle_id",
        "metal_bundle_version",
    ] {
        require(
            !get(m, key)?.is_empty(),
            "missing exact device/framework identity",
        )?;
    }
    let id = get(m, "registry_id")?;
    require(
        id.len() == 16 && u64::from_str_radix(id, 16).is_ok(),
        "invalid registry ID",
    )?;
    Ok(format!(
        "{}|{}|{}|{}|{}",
        get(m, "device_name")?,
        id,
        get(m, "os_version_and_build")?,
        get(m, "metal_bundle_id")?,
        get(m, "metal_bundle_version")?
    ))
}

fn command_rows(s: &str, bytes: usize) -> Result<()> {
    let mut lines = s.lines();
    require(lines.next() == Some(COMMAND_HEADER), "command header")?;
    let mut seen = BTreeSet::new();
    for line in lines {
        let f: Vec<_> = line.split('\t').collect();
        require(f.len() == 11, "command field count")?;
        let repeat: u32 = f[1].parse().map_err(|_| "command repeat")?;
        require(KERNELS.contains(&f[0]) && repeat < 3, "unknown command")?;
        require(seen.insert((f[0], repeat)), "duplicate command")?;
        require(
            f[2..6] == ["4", "1", "1", "1"],
            "command did not complete cleanly",
        )?;
        require(f[6] == bytes.to_string(), "readback length receipt")?;
        for v in &f[7..9] {
            require(
                v.parse::<u64>().is_ok_and(|x| x > 0),
                "pipeline width metadata",
            )?;
        }
        // Timings are diagnostics only. Zero does not imply CPU execution or failure.
        for v in &f[9..11] {
            require(
                !v.is_empty()
                    && v.bytes()
                        .all(|b| b.is_ascii_digit() || b".eE+-".contains(&b)),
                "invalid GPU timing",
            )?;
        }
    }
    require(seen.len() == KERNELS.len() * 3, "missing command receipt")
}

fn fields(kind: usize) -> Vec<(usize, &'static str)> {
    match kind {
        0..=5 => vec![(10, "result")],
        7 => vec![
            (10, "length2"),
            (11, "inverse"),
            (12, "unit_x"),
            (13, "unit_y"),
            (14, "normal_x"),
            (15, "normal_y"),
            (16, "projection"),
            (17, "fabs"),
            (18, "ordered_lt"),
            (19, "candidate_x"),
            (20, "candidate_y"),
            (21, "chosen_x"),
            (22, "chosen_y"),
            (23, "saturate"),
            (24, "sqrt_saturate"),
        ],
        8 => vec![
            (21, "chosen_x"),
            (22, "chosen_y"),
            (23, "saturate"),
            (24, "sqrt_saturate"),
        ],
        _ => vec![],
    }
}

fn validate_raw(raw: &[u8], kind: usize, c: &[Case]) -> Result<Vec<[u32; WORDS]>> {
    require(kind < KERNELS.len(), "unknown kernel index")?;
    let expected = c.len() * WORDS * 4 + GUARD * 2;
    require(raw.len() == expected, "wrong raw length")?;
    require(
        raw[..GUARD]
            .iter()
            .chain(&raw[expected - GUARD..])
            .all(|b| *b == 0xa5),
        "buffer guard changed",
    )?;
    let mut result = Vec::new();
    let active: BTreeSet<_> = fields(kind).into_iter().map(|(i, _)| i).collect();
    for (row, c) in c.iter().enumerate() {
        let mut words = [0; WORDS];
        for (i, w) in words.iter_mut().enumerate() {
            let at = GUARD + (row * WORDS + i) * 4;
            *w = u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
        }
        if kind == 6 {
            require(
                words.iter().all(|w| *w == POISON),
                "noop unexpectedly wrote",
            )?;
        } else {
            require(
                words[0] == (0x51ad0000 | (kind as u32 + 1)) && words[1] == row as u32,
                "missing/wrong GPU write marker",
            )?;
            require(
                words[2..6] == c.a && words[6..10] == c.b,
                "raw input echo mismatch",
            )?;
            for (i, w) in words.iter().enumerate().skip(10) {
                if !active.contains(&i) {
                    require(*w == POISON, "inactive output word overwritten")?;
                }
            }
            if kind == 7 {
                require(words[18] <= 1, "comparison is not a Boolean")?;
            }
            // Only exact finite arithmetic controls; never predict exceptional dot.
            if row == 0 && kind < 6 {
                let exact = [
                    0x41880000, 0x42180000, 0x428c0000, 0x41600000, 0x41c00000, 0x42100000,
                ];
                require(words[10] == exact[kind], "finite arithmetic anchor failed")?;
            }
            if c.name == "inactive_tail_poison2" && kind == 0 {
                require(words[10] == 0x41300000, "dot2 consumed inactive lanes")?;
            }
            if c.name == "inactive_tail_poison3" && kind == 1 {
                require(words[10] == 0x42180000, "dot3 consumed inactive lane")?;
            }
        }
        result.push(words);
    }
    Ok(result)
}

type CaptureKey = (String, u32, usize, u32);
fn check(root: &Path, output: Option<&Path>) -> Result<()> {
    let c = cases();
    require(
        read(&root.join("inputs.bin"), 4096)? == input_bytes(&c),
        "cohort bytes differ from reviewed generator",
    )?;
    let toolchain = metadata(&text(&root.join("toolchain.tsv"), 16384)?)?;
    for key in [
        "machine_arch",
        "os_version",
        "os_build",
        "sdk_version",
        "clang",
        "rustc",
        "xcode_version",
        "xcode_build",
    ] {
        require(
            !get(&toolchain, key)?.is_empty(),
            "incomplete toolchain metadata",
        )?;
    }
    require(
        ["arm64", "x86_64"].contains(&get(&toolchain, "machine_arch")?),
        "unsupported native host architecture",
    )?;
    let mut captures: BTreeMap<CaptureKey, Vec<[u32; WORDS]>> = BTreeMap::new();
    let mut identity: Option<String> = None;
    let mut observations = String::from(
        "profile\tbuild_request\trepeat\tkernel\trow\tlabel\tgroup\tfield\tbits\tclass\n",
    );
    for profile in PROFILES {
        for build in 0..2 {
            let dir = root.join(format!("{profile}-b{build}"));
            let m = metadata(&text(&dir.join("metadata.tsv"), 32768)?)?;
            let device = check_meta(&m, profile, build, c.len())?;
            require(
                get(&m, "os_version_and_build")?.contains(get(&toolchain, "os_build")?),
                "OS build binding",
            )?;
            if let Some(first) = &identity {
                require(
                    first == &device,
                    "device/OS/framework changed within cohort",
                )?;
            } else {
                identity = Some(device);
            }
            require(
                read(&dir.join("loaded-source.metal"), 32768)? == SOURCE,
                "runtime source binding failed",
            )?;
            require(
                read(&dir.join("loaded-input.bin"), 4096)? == input_bytes(&c),
                "runtime input binding failed",
            )?;
            command_rows(
                &text(&dir.join("commands.tsv"), 32768)?,
                c.len() * WORDS * 4 + GUARD * 2,
            )?;
            for (kind, kernel) in KERNELS.into_iter().enumerate() {
                for repeat in 0..3 {
                    let rows = validate_raw(
                        &read(&dir.join(format!("{kernel}-{repeat}.bin")), 8192)?,
                        kind,
                        &c,
                    )
                    .map_err(|e| format!("{profile}/b{build}/{kernel}/r{repeat}: {e}"))?;
                    for (row, w) in rows.iter().enumerate() {
                        for (i, name) in fields(kind) {
                            let label = if i == 18 {
                                format!("bool:{}", w[i])
                            } else {
                                class(w[i])
                            };
                            writeln!(observations,"{profile}\t{build}\t{repeat}\t{kernel}\t{row}\t{}\t{}\t{name}\t{:08x}\t{label}",c[row].name,c[row].group,w[i]).unwrap();
                        }
                    }
                    captures.insert((profile.to_owned(), build, kind, repeat), rows);
                }
            }
        }
    }
    // Native arithmetic substitution and no-op negative controls must be caught
    // by the SAME validator even if their tags are relabeled as a nominal dot.
    for profile in PROFILES {
        for build in 0..2 {
            for repeat in 0..3 {
                let dir = root.join(format!("{profile}-b{build}"));
                let noop = read(&dir.join(format!("noop-{repeat}.bin")), 8192)?;
                require(validate_raw(&noop, 0, &c).is_err(), "no-op accepted as dot")?;
                for kind in 0..3 {
                    let mut wrong = read(
                        &dir.join(format!("{}-{repeat}.bin", KERNELS[kind + 3])),
                        8192,
                    )?;
                    for row in 0..c.len() {
                        let at = GUARD + row * WORDS * 4;
                        wrong[at..at + 4]
                            .copy_from_slice(&(0x51ad0001 + kind as u32).to_le_bytes());
                    }
                    require(
                        validate_raw(&wrong, kind, &c).is_err(),
                        "wrong arithmetic accepted as dot",
                    )?;
                }
            }
        }
    }
    let mut composition = String::from("profile\tbuild_request\trepeat\trow\tlabel\tgroup\tprojection_bits\tfabs_bits\tgate\tcandidate_x\tcandidate_y\tchosen_x\tchosen_y\tsink_x\tsink_y\tnan_discard_exact_bits\tchosen_finite\tsink_matches_trace\n");
    let mut trace_sink_diffs = 0;
    let mut nan_discard_rows = 0;
    for profile in PROFILES {
        for build in 0..2 {
            for repeat in 0..3 {
                let trace = &captures[&(profile.to_owned(), build, 7, repeat)];
                let sink = &captures[&(profile.to_owned(), build, 8, repeat)];
                for (row, case) in c.iter().enumerate() {
                    let t = trace[row];
                    let s = sink[row];
                    // Returning an original NaN is NOT a safely discarded NaN candidate.
                    let discarded = nan(t[16])
                        && t[18] == 0
                        && case.b[..2].iter().all(|x| finite(*x))
                        && t[21..23] == case.b[..2];
                    let same = t[21..25] == s[21..25];
                    if case.group == "composition_control" {
                        require(
                            t[10] == 0x3f800000
                                && [t[11], t[16], t[17], t[21], t[22], s[21], s[22]]
                                    .into_iter()
                                    .all(finite),
                            "finite composition control invalid",
                        )?;
                        if case.name == "unit_vector_inside" {
                            require(
                                t[18] == 1 && t[21..23] != case.b[..2] && s[21..23] != case.b[..2],
                                "select control silently passed original delta",
                            )?;
                        } else {
                            require(
                                t[18] == 0 && t[21..23] == case.b[..2] && s[21..23] == case.b[..2],
                                "select control silently chose candidate",
                            )?;
                        }
                    }
                    nan_discard_rows += usize::from(discarded);
                    trace_sink_diffs += usize::from(!same);
                    writeln!(composition,"{profile}\t{build}\t{repeat}\t{row}\t{}\t{}\t{:08x}\t{:08x}\t{}\t{:08x}\t{:08x}\t{:08x}\t{:08x}\t{:08x}\t{:08x}\t{}\t{}\t{}",case.name,case.group,t[16],t[17],t[18],t[19],t[20],t[21],t[22],s[21],s[22],u8::from(discarded),u8::from(finite(t[21])&&finite(t[22])),u8::from(same)).unwrap();
                }
            }
        }
    }
    let mut differences = String::from("axis\tprofile\tbuild_request\trepeat\tkernel\trow\tlabel\tfield\tbaseline_bits\tobserved_bits\n");
    let mut repeat_build_diffs = 0;
    let mut profile_diffs = 0;
    for profile in PROFILES {
        for build in 0..2 {
            for repeat in 0..3 {
                for kind in 0..9 {
                    let current = &captures[&(profile.to_owned(), build, kind, repeat)];
                    let stable = &captures[&(profile.to_owned(), 0, kind, 0)];
                    let precise = &captures[&("false".to_owned(), build, kind, repeat)];
                    for row in 0..c.len() {
                        for (word, name) in fields(kind) {
                            for (axis, baseline, counter) in [
                                ("repeat_or_build", stable, &mut repeat_build_diffs),
                                ("profile_vs_false", precise, &mut profile_diffs),
                            ] {
                                if current[row][word] != baseline[row][word] {
                                    *counter += 1;
                                    writeln!(differences,"{axis}\t{profile}\t{build}\t{repeat}\t{}\t{row}\t{}\t{name}\t{:08x}\t{:08x}",KERNELS[kind],c[row].name,baseline[row][word],current[row][word]).unwrap();
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let summary = format!("schema\tnative-dot-summary-v1\nstatus\tintegrity_checked_observations_only\nmodel_qualified\t0\ncase_count\t{}\nprofiles\tfalse,true,default\nbuild_requests_per_profile\t2\nrepeats_per_kernel\t3\ncompleted_native_commands\t162\nno_op_relabel_rejections\t18\nwrong_arithmetic_relabel_rejections\t54\nnan_discard_exact_rows\t{nan_discard_rows}\ntrace_sink_different_rows\t{trace_sink_diffs}\nrepeat_or_build_different_words\t{repeat_build_diffs}\nprofile_different_words\t{profile_diffs}\nfinite_overflow_policy_decided\t0\n",c.len());
    for (name, contents) in [
        ("observations.tsv", &observations),
        ("composition.tsv", &composition),
        ("differences.tsv", &differences),
        ("summary.tsv", &summary),
    ] {
        if let Some(out) = output {
            write_new(&out.join(name), contents.as_bytes())?;
        } else {
            require(
                read(&root.join("report").join(name), 8 * 1024 * 1024)? == contents.as_bytes(),
                "derived report differs from raw reconstruction",
            )?;
        }
    }
    print!("{summary}");
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let result = match args.get(1).and_then(|s| s.to_str()) {
        Some("prepare") if args.len()==3 => prepare(Path::new(&args[2])),
        Some("check") if args.len()==4 => check(Path::new(&args[2]),Some(Path::new(&args[3]))),
        Some("verify") if args.len()==3 => check(Path::new(&args[2]),None),
        _ => Err("usage: oracle prepare EXISTING_NEW_DIR | check CAPTURE_DIR EXISTING_NEW_REPORT_DIR | verify CAPTURE_DIR".into()),
    };
    if let Err(error) = result {
        eprintln!("validation refused: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // In-memory parser fixtures ONLY: never written or represented as GPU evidence.
    fn fixture(kind: usize) -> Vec<u8> {
        let c = cases();
        let mut raw = vec![0xa5; c.len() * WORDS * 4 + GUARD * 2];
        if kind == 6 {
            return raw;
        }
        for (row, c) in c.iter().enumerate() {
            let mut w = [POISON; WORDS];
            w[0] = 0x51ad0001 + kind as u32;
            w[1] = row as u32;
            w[2..6].copy_from_slice(&c.a);
            w[6..10].copy_from_slice(&c.b);
            if kind < 6 {
                w[10] = 0;
            }
            if kind == 7 {
                w[18] = 0;
            }
            if row == 0 && kind < 6 {
                w[10] = [
                    0x41880000, 0x42180000, 0x428c0000, 0x41600000, 0x41c00000, 0x42100000,
                ][kind];
            }
            if c.name == "inactive_tail_poison2" && kind == 0 {
                w[10] = 0x41300000;
            }
            if c.name == "inactive_tail_poison3" && kind == 1 {
                w[10] = 0x42180000;
            }
            for (i, v) in w.into_iter().enumerate() {
                let at = GUARD + (row * WORDS + i) * 4;
                raw[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        raw
    }
    #[test]
    fn parser_only_valid_layouts() {
        for k in 0..9 {
            validate_raw(&fixture(k), k, &cases()).unwrap();
        }
    }
    #[test]
    fn parser_only_noop_rejected_as_dot() {
        assert!(validate_raw(&fixture(6), 0, &cases()).is_err());
    }
    #[test]
    fn parser_only_wrong_arithmetic_rejected_even_with_correct_tag() {
        let mut raw = fixture(3);
        for row in 0..cases().len() {
            let at = GUARD + row * WORDS * 4;
            raw[at..at + 4].copy_from_slice(&0x51ad0001u32.to_le_bytes());
        }
        assert!(validate_raw(&raw, 0, &cases()).is_err());
    }
    #[test]
    fn parser_only_guards_echo_tag_and_padding() {
        for at in [0, GUARD, GUARD + 8, GUARD + 44, fixture(0).len() - 1] {
            let mut raw = fixture(0);
            raw[at] ^= 1;
            assert!(validate_raw(&raw, 0, &cases()).is_err(), "{at}");
        }
    }
    #[test]
    fn parser_only_truncation_and_extra_bytes() {
        let raw = fixture(0);
        assert!(validate_raw(&raw[..raw.len() - 1], 0, &cases()).is_err());
        let mut extra = raw;
        extra.push(0);
        assert!(validate_raw(&extra, 0, &cases()).is_err());
    }
    #[test]
    fn nan_labels_do_not_canonicalize() {
        assert_eq!(class(0x7fc00042), "+qnan:fraction=400042");
        assert_eq!(class(0xff800001), "-snan:fraction=000001");
        assert_eq!(class(0x80000000), "-zero");
        assert_eq!(class(1), "+subnormal");
        assert_eq!(class(0x7f800000), "+infinity");
        assert!(!finite(0xffc00001));
    }
    #[test]
    fn cohort_is_finite_in_size_not_finite_only_in_value() {
        let c = cases();
        assert_eq!(c.len(), 26);
        assert_eq!(input_bytes(&c).len(), 832);
        assert_eq!(c[2].a, [0x80000000, 0, 0, 0]);
        assert_eq!(c[2].b, [0x7fc00000, 0x7fc00000, 0, 0]);
        assert_eq!(c[6].a, [0, 0x80000000, 0, 0]);
        assert_eq!(c[6].b, [0, 0x80000000, 0, 0]);
        assert_eq!(c.iter().filter(|c| c.group == "finite_overflow").count(), 3);
        for c in c.iter().filter(|c| c.group == "finite_overflow") {
            assert!(c.a.into_iter().chain(c.b).all(finite));
        }
    }
    #[test]
    fn metadata_duplicates_and_escapes() {
        assert!(metadata("x\t1\nx\t2\n").is_err());
        assert!(metadata("x\t\\q\n").is_err());
        assert_eq!(
            metadata("name\tGPU\\tA\\\\B\n").unwrap()["name"],
            "GPU\tA\\B"
        );
    }
    fn receipts() -> String {
        let mut s = format!("{COMMAND_HEADER}\n");
        for name in KERNELS {
            for r in 0..3 {
                writeln!(s, "{name}\t{r}\t4\t1\t1\t1\t3200\t32\t1024\t0\t0").unwrap();
            }
        }
        s
    }
    #[test]
    fn parser_only_complete_command_matrix() {
        command_rows(&receipts(), 3200).unwrap();
    }
    #[test]
    fn parser_only_missing_duplicate_failed_commands() {
        let s = receipts();
        assert!(command_rows(&s.replacen("\t4\t1\t1\t1\t", "\t5\t0\t1\t1\t", 1), 3200).is_err());
        assert!(command_rows(&s.lines().take(27).collect::<Vec<_>>().join("\n"), 3200).is_err());
        let mut dup = s.clone();
        dup.push_str(s.lines().nth(1).unwrap());
        dup.push('\n');
        assert!(command_rows(&dup, 3200).is_err());
    }
    fn meta_fixture(profile: &str) -> BTreeMap<String, String> {
        let mut m: BTreeMap<String, String> = [
            ("schema", "native-dot-v1"),
            ("backend", "native_metal"),
            ("cpu_fallback", "0"),
            ("selection", "system_default_only"),
            ("compiler_route", "newLibraryWithSource"),
            ("other_math_setters_called", "0"),
            ("buffer_storage", "shared"),
            ("guard_bytes_each_side", "64"),
            ("row_words", "32"),
            ("completed_commands", "27"),
            ("status", "native_bytes_observed_not_model_qualification"),
            ("host_pointer_bits", "64"),
            ("build_request", "0"),
            ("fast_math_before", "1"),
            ("fast_math_after", "1"),
            ("fast_math_setter_called", "0"),
            ("unified_memory", "1"),
            ("low_power", "1"),
            ("removable", "0"),
            ("headless", "0"),
            ("language_version_getter", "0"),
            ("library_type", "0"),
            ("max_threadgroup_memory", "32768"),
            ("max_buffer_length", "1073741824"),
            ("max_threads_x", "1024"),
            ("max_threads_y", "1024"),
            ("max_threads_z", "1024"),
            ("math_mode_getter", "unavailable"),
            ("math_functions_getter", "unavailable"),
            ("device_name", "PARSER_FIXTURE_NOT_A_DEVICE"),
            ("os_version_and_build", "PARSER_FIXTURE"),
            ("metal_bundle_id", "PARSER_FIXTURE"),
            ("metal_bundle_version", "PARSER_FIXTURE"),
            ("registry_id", "0000000000000001"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        m.insert("profile".into(), profile.into());
        m.insert("case_count".into(), cases().len().to_string());
        m.insert("source_bytes".into(), SOURCE.len().to_string());
        if profile != "default" {
            m.insert("fast_math_setter_called".into(), "1".into());
        }
        if profile == "false" {
            m.insert("fast_math_after".into(), "0".into());
        }
        m
    }
    #[test]
    fn parser_only_three_profiles_are_distinct() {
        for p in PROFILES {
            check_meta(&meta_fixture(p), p, 0, cases().len()).unwrap();
        }
        let mut m = meta_fixture("default");
        m.insert("fast_math_setter_called".into(), "1".into());
        assert!(check_meta(&m, "default", 0, cases().len()).is_err());
        let mut m = meta_fixture("false");
        m.insert("fast_math_after".into(), "1".into());
        assert!(check_meta(&m, "false", 0, cases().len()).is_err());
    }
    #[test]
    fn parser_only_missing_or_failed_native_metadata_rejected() {
        for k in [
            "device_name",
            "registry_id",
            "os_version_and_build",
            "metal_bundle_version",
            "math_mode_getter",
        ] {
            let mut m = meta_fixture("true");
            m.remove(k);
            assert!(check_meta(&m, "true", 0, cases().len()).is_err());
        }
        let mut m = meta_fixture("true");
        m.insert("failure_phase".into(), "command".into());
        assert!(check_meta(&m, "true", 0, cases().len()).is_err());
    }
}
