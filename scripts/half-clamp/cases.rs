// Public synthetic scalar half clamp: observation, not an assumed NaN endpoint.
// Reuses public clamp controls and the reviewed guarded/source-bound Rust harness.
#![forbid(unsafe_code)]
use std::{collections::BTreeMap, env, fs, io::Write, path::Path, process::Command};
type Result<T> = std::result::Result<T, String>;
const SOURCE: [&str; 5] = [
    ".github/workflows/half-clamp.yml",
    "scripts/half-clamp/cases.rs",
    "scripts/half-clamp/probe.metal",
    "scripts/half-clamp/probe.swift",
    "scripts/half-clamp/run.sh",
];
const G: usize = 64;
const RECORD: usize = 32;
const MAGIC: u32 = 0x6863_6c00;
const POISON: u8 = 0xcd;
const NANS: usize = 2046;
const WIDTH: usize = 64;
const COUNT: usize = 24848;
const CONTROL_ROW: usize = NANS + 11;
const LIMIT: u64 = 4 * 1024 * 1024;
const BOUNDS: [(u16, u16); 12] = [
    (0, 0x3c00),
    (0x3400, 0x3a00),
    (0x4000, 0x4200),
    (0xc200, 0xc000),
    (0xb800, 0x3800),
    (0x3800, 0x3800),
    (0x8000, 0),
    (1, 0x03ff),
    (0x83ff, 0x8001),
    (0, 0x8000),
    (0xb800, 0xb800),
    (0x8001, 1),
];
const FINITE: [u16; 24] = [
    0, 0x8000, 1, 0x8001, 0x03ff, 0x83ff, 0x0400, 0x8400, 0x3400, 0x3800, 0x3a00, 0x3c00, 0x4000,
    0x4200, 0xb400, 0xb800, 0xba00, 0xbc00, 0xc000, 0xc200, 0x7bff, 0xfbff, 0x7c00, 0xfc00,
];
const COMPLETE: &str = "schema=1\nstatus=0\ncommands=3\ncaptures=3\ncases=24848\n";
const EXIT_OK: &str = "{\"exit\":0,\"phase\":\"complete\"}\n";
const NEGATIVE:&str="{\"wrong_expected_rejected\":true,\"native_bytes_unchanged\":true,\"row\":2057,\"operator\":\"dynamic_clamp\",\"exit\":2}\n";
const RECEIPT_HEADER:&str="tag\tcapture\tprofile\tstatus\terror\tgpu_start\tgpu_end\tformat\twidth\theight\tsamples\twrite_mask\tblending\trasterization\talpha_to_coverage\talpha_to_one\tload\tstore\tdraws\tvertices\tpso_created\tcases\tinput_bytes\ttrace_bytes";
const COMPILER_HEADER:&str="profile\tfast_math\tmath_mode\tmath_functions\tlanguage\toptimization\tpreserve_invariance\tmode_matches\tfunctions_match\tlanguage_match\tdenorm_override\tdefault_fast\tdefault_mode\tdefault_functions\tdefault_language";
#[derive(Clone, Copy, Debug)]
struct Case {
    a: u16,
    b: u16,
    lo: u16,
    hi: u16,
    route: u32,
    bound: u32,
}
fn need(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn nan(x: u16) -> bool {
    x & 0x7c00 == 0x7c00 && x & 1023 != 0
}
fn subnormal(x: u16) -> bool {
    x & 0x7c00 == 0 && x & 1023 != 0
}
fn finite(x: u16) -> bool {
    x & 0x7c00 != 0x7c00
}
fn decode(h: u16) -> f64 {
    let s = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
    let e = (h >> 10) & 31;
    let m = h & 1023;
    s * match e {
        0 => f64::from(m) * 2f64.powi(-24),
        31 => {
            if m == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => f64::from(1024 + m) * 2f64.powi(i32::from(e) - 25),
    }
}
fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (bound, (lo, hi)) in BOUNDS.into_iter().enumerate() {
        assert!(finite(lo) && finite(hi) && decode(lo) <= decode(hi));
        for sign in [0, 0x8000] {
            for payload in 1..=1023 {
                out.push(Case {
                    a: sign | 0x7c00 | payload,
                    b: 0x3c00,
                    lo,
                    hi,
                    route: 0,
                    bound: bound as u32,
                });
            }
        }
        for a in FINITE {
            out.push(Case {
                a,
                b: 0x3c00,
                lo,
                hi,
                route: 0,
                bound: bound as u32,
            });
        }
    }
    for (a, b, bound) in [
        (0, 0, 0),
        (0x8000, 0, 0),
        (0x7c00, 0x7c00, 0),
        (0xfc00, 0x7c00, 0),
        (0, 0, 1),
        (0x7c00, 0x7c00, 3),
        (0x8000, 0x8000, 7),
        (0xfc00, 0xfc00, 8),
    ] {
        let (lo, hi) = BOUNDS[bound];
        out.push(Case {
            a,
            b,
            lo,
            hi,
            route: 1,
            bound: bound as u32,
        });
    }
    assert_eq!(out.len(), COUNT);
    out
}
fn inputs(cases: &[Case]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cases.len() * 16);
    for c in cases {
        for v in [c.a, c.b, c.lo, c.hi] {
            out.extend(v.to_le_bytes());
        }
        out.extend(c.route.to_le_bytes());
        out.extend(c.bound.to_le_bytes());
    }
    out
}
fn guarded(b: &[u8]) -> Vec<u8> {
    [vec![POISON; G], b.to_vec(), vec![POISON; G]].concat()
}
fn plan() -> String {
    assert_eq!(COUNT.div_ceil(WIDTH), 389);
    let bounds = BOUNDS
        .iter()
        .map(|(l, h)| format!("[{l},{h}]"))
        .collect::<Vec<_>>()
        .join(",");
    format!(concat!("{{\"schema\":1,\"count\":24848,\"width\":64,\"height\":389,\"guard\":64,\"input_stride\":16,\"trace_stride\":32,",
      "\"bounds\":[{}],\"direct_nan_encodings_per_bound\":2046,\"direct_control_words_per_bound\":24,\"composition_controls\":8,",
      "\"source_order\":\"per bound: positive NaN payloads 1..1023, negative NaN payloads 1..1023, control words; then eight synthetic divide controls\",",
      "\"control_words\":{:?},\"captures\":[{{\"name\":\"fast\",\"profile\":\"fast\",\"tag\":1,\"skip\":4294967295}},{{\"name\":\"precise\",\"profile\":\"precise\",\"tag\":2,\"skip\":4294967295}},{{\"name\":\"fast-missing\",\"profile\":\"fast\",\"tag\":3,\"skip\":0}}],",
      "\"values\":[\"actual_half_input\",\"dynamic_half_clamp\",\"literal_half_clamp_0_1\",\"separate_half_saturate_control\"],\"nan_endpoint_assumed\":false}}\n"),bounds,FINITE)
}
fn read16(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes(b[p..p + 2].try_into().unwrap())
}
fn read32(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
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
fn generate(root: &Path) -> Result<()> {
    write_new(&root.join("inputs.bin"), &inputs(&cases()))?;
    write_new(&root.join("cases.json"), plan().as_bytes())?;
    println!("cases={COUNT} raw_nan_encodings_per_bound={NANS} bounds=12 command_buffers=3");
    Ok(())
}
fn value(trace: &[u8], row: usize, index: usize) -> u16 {
    read16(trace, G + row * RECORD + 16 + index * 2)
}
fn marker(tag: u32, c: Case) -> u32 {
    MAGIC ^ (tag << 16) ^ (c.route << 8) ^ c.bound
}
fn direct_nan_index(bound: u32, word: u16) -> Option<usize> {
    if !nan(word) || bound >= 12 {
        return None;
    }
    Some(
        bound as usize * (NANS + 24)
            + usize::from(word & 0x8000 != 0) * 1023
            + usize::from(word & 1023)
            - 1,
    )
}
// Only established ordinary numeric controls are hard expectations. NaNs have
// NO expected endpoint. Subnormals and fast-profile Inf controls are diagnostic.
fn expected_control(c: Case, profile: &str, op: usize) -> Option<u16> {
    if c.route != 0 || nan(c.a) || subnormal(c.a) || (profile == "fast" && !finite(c.a)) {
        return None;
    }
    let (lo, hi) = if op == 0 { (c.lo, c.hi) } else { (0, 0x3c00) };
    if subnormal(lo) || subnormal(hi) {
        return None;
    }
    Some(if decode(c.a) < decode(lo) {
        lo
    } else if decode(c.a) > decode(hi) {
        hi
    } else {
        c.a
    })
}
fn expect_word(trace: &[u8], row: usize, op: usize, want: u16) -> Result<()> {
    let actual = value(trace, row, op + 1);
    need(
        if want & 0x7fff == 0 {
            actual & 0x7fff == 0
        } else {
            actual == want
        },
        format!("ordinary-value row={row} op={op}"),
    )
}
fn guards(trace: &[u8]) -> Result<()> {
    need(trace.len() == G * 2 + COUNT * RECORD, "trace length")?;
    need(
        trace[..G]
            .iter()
            .chain(trace[G + COUNT * RECORD..].iter())
            .all(|b| *b == POISON),
        "trace guards",
    )
}
fn validate_capture(trace: &[u8], profile: &str, tag: u32, cases: &[Case]) -> Result<usize> {
    guards(trace)?;
    let mut checked = 0;
    for (i, c) in cases.iter().enumerate() {
        let p = G + i * RECORD;
        need(
            [read32(trace, p), read32(trace, p + 4)] == [marker(tag, *c), i as u32],
            format!("trace-marker row={i}"),
        )?;
        need(
            (0..4)
                .map(|k| read16(trace, p + 8 + k * 2))
                .collect::<Vec<_>>()
                == [c.a, c.b, c.lo, c.hi],
            format!("operand-echo row={i}"),
        )?;
        for k in 0..4 {
            need(
                value(trace, i, k) ^ read16(trace, p + 24 + k * 2) == 0xffff,
                format!("result-integrity row={i} slot={k}"),
            )?;
        }
        if c.route == 0 {
            need(value(trace, i, 0) == c.a, format!("raw-half-input row={i}"))?;
        }
        for op in 0..3 {
            if let Some(w) = expected_control(*c, profile, op) {
                expect_word(trace, i, op, w)?;
                checked += 1;
            }
        }
    }
    Ok(checked)
}
fn text(root: &Path, name: &str) -> Result<String> {
    String::from_utf8(read_bounded(&root.join(name))?).map_err(|_| "UTF-8".into())
}
fn enum_check(s: &str) -> Result<BTreeMap<String, u32>> {
    let mut l = s.lines();
    need(l.next() == Some("symbol\tvalue"), "SDK header")?;
    let mut m = BTreeMap::new();
    for line in l {
        let (k, v) = line.split_once('\t').ok_or("SDK fields")?;
        need(
            m.insert(k.to_string(), v.parse::<u32>().map_err(|_| "SDK integer")?)
                .is_none(),
            "SDK duplicate",
        )?;
    }
    need(m.len() == 9, "SDK count")?;
    for (k, v) in [
        ("rgba8Unorm", 70),
        ("completed", 4),
        ("language24", 131076),
        ("load_dontcare", 0),
        ("store_dontcare", 0),
    ] {
        need(m.get(k) == Some(&v), "SDK numeric enum")?;
    }
    for k in [
        "mode_fast",
        "mode_safe",
        "functions_fast",
        "functions_precise",
    ] {
        need(m.contains_key(k), "SDK math enum missing")?;
    }
    need(
        m["mode_fast"] != m["mode_safe"] && m["functions_fast"] != m["functions_precise"],
        "SDK math distinctions",
    )?;
    Ok(m)
}
fn compiler_check(s: &str, e: &BTreeMap<String, u32>) -> Result<()> {
    let l: Vec<_> = s.lines().collect();
    need(
        l.len() == 3 && l[0] == COMPILER_HEADER,
        "compiler coverage/header",
    )?;
    for (i, profile) in ["fast", "precise"].into_iter().enumerate() {
        let f: Vec<_> = l[i + 1].split('\t').collect();
        need(f.len() == 15 && f[0] == profile, "compiler profile fields")?;
        let n: Vec<u32> = f[1..]
            .iter()
            .map(|v| v.parse::<u32>().map_err(|_| "compiler integer".to_string()))
            .collect::<Result<_>>()?;
        let fast = profile == "fast";
        need(
            n[0] == u32::from(fast)
                && n[1] == e[if fast { "mode_fast" } else { "mode_safe" }]
                && n[2]
                    == e[if fast {
                        "functions_fast"
                    } else {
                        "functions_precise"
                    }]
                && n[3] == 131076,
            "compiler effective getters",
        )?;
        need(
            n[5] <= 1 && n[6..10] == [1, 1, 1, 0] && n[10] <= 1,
            "compiler booleans/denorm scope",
        )?;
    }
    Ok(())
}
fn receipt_check(s: &str) -> Result<()> {
    let l: Vec<_> = s.lines().collect();
    need(
        l.len() == 4 && l[0] == RECEIPT_HEADER,
        "receipt coverage/header",
    )?;
    for (i, (capture, profile)) in [
        ("fast", "fast"),
        ("precise", "precise"),
        ("fast-missing", "fast"),
    ]
    .into_iter()
    .enumerate()
    {
        let f: Vec<_> = l[i + 1].split('\t').collect();
        need(
            f.len() == 24 && f[1] == capture && f[2] == profile,
            "receipt identity/fields",
        )?;
        let n = |j: usize| {
            f[j].parse::<u32>()
                .map_err(|_| "receipt integer".to_string())
        };
        need(
            n(0)? == i as u32 + 1 && n(3)? == 4 && n(4)? == 0,
            "command completion/error",
        )?;
        let start: f64 = f[5].parse().map_err(|_| "GPU time")?;
        let end: f64 = f[6].parse().map_err(|_| "GPU time")?;
        need(
            start.is_finite() && end.is_finite() && start >= 0.0 && end >= start,
            "GPU timing",
        )?;
        let want = [
            70,
            64,
            389,
            1,
            15,
            0,
            1,
            0,
            0,
            0,
            0,
            1,
            3,
            1,
            COUNT as u32,
            (COUNT * 16 + G * 2) as u32,
            (COUNT * RECORD + G * 2) as u32,
        ];
        for (k, v) in want.into_iter().enumerate() {
            need(n(k + 7)? == v, "pipeline/draw/buffer getters")?;
        }
    }
    Ok(())
}
#[derive(Default)]
struct Histogram {
    counts: BTreeMap<u16, usize>,
    n: usize,
    lo: usize,
    hi: usize,
    both: usize,
    input: usize,
    nans: usize,
    infs: usize,
    poszero: usize,
    negzero: usize,
}
impl Histogram {
    fn add(&mut self, actual: u16, input: u16, lo: u16, hi: u16) {
        *self.counts.entry(actual).or_default() += 1;
        self.n += 1;
        self.lo += usize::from(actual == lo);
        self.hi += usize::from(actual == hi);
        self.both += usize::from(actual == lo && actual == hi);
        self.input += usize::from(actual == input);
        self.nans += usize::from(nan(actual));
        self.infs += usize::from(actual & 0x7fff == 0x7c00);
        self.poszero += usize::from(actual == 0);
        self.negzero += usize::from(actual == 0x8000);
    }
}
struct Checked {
    json: String,
    nans: String,
    controls: String,
    wrong_expected_rejected: bool,
}
fn acquire(root: &Path, expected: &str, source: &Path) -> Result<Checked> {
    bounds(root)?;
    binding(root, expected, source)?;
    need(
        read_bounded(&root.join("complete.txt"))? == COMPLETE.as_bytes(),
        "native completion",
    )?;
    let c = cases();
    let input = inputs(&c);
    need(
        read_bounded(&root.join("inputs.bin"))? == input
            && read_bounded(&root.join("inputs-after.bin"))? == guarded(&input),
        "input/guard integrity",
    )?;
    need(
        read_bounded(&root.join("cases.json"))? == plan().as_bytes(),
        "case plan",
    )?;
    let enums = enum_check(&text(root, "sdk-enums.tsv")?)?;
    compiler_check(&text(root, "compiler.tsv")?, &enums)?;
    receipt_check(&text(root, "receipts.tsv")?)?;
    let fast = read_bounded(&root.join("fast.bin"))?;
    let precise = read_bounded(&root.join("precise.bin"))?;
    let missing = read_bounded(&root.join("fast-missing.bin"))?;
    let checks = [
        validate_capture(&fast, "fast", 1, &c)?,
        validate_capture(&precise, "precise", 2, &c)?,
    ];
    need(
        validate_capture(&missing, "fast", 3, &c).err() == Some("trace-marker row=0".into()),
        "missing-row ordinary rejection",
    )?;
    guards(&missing)?;
    need(
        missing[G..G + RECORD].iter().all(|v| *v == POISON),
        "missing row unexpectedly written",
    )?;
    for i in 1..COUNT {
        let p = G + i * RECORD;
        need(
            read32(&missing, p) == marker(3, c[i])
                && missing[p + 4..p + RECORD] == fast[p + 4..p + RECORD],
            "negative altered another row",
        )?;
    }
    let wrong_expected_rejected = expect_word(&fast, CONTROL_ROW, 0, 0x3c01).err()
        == Some(format!("ordinary-value row={CONTROL_ROW} op=0"));
    need(wrong_expected_rejected, "wrong expected passed")?;
    let mut nan_table=String::from("profile\tbound\toperator\tinput_kind\tcount\tlo\thi\tequal_lo\tequal_hi\tequal_both\tequal_input_bits\tnan_outputs\tinf_outputs\tpositive_zero\tnegative_zero\tdistinct_output_words\tfirst_16_word_counts\n");
    let mut controls=String::from("profile\trow\tbound\troute\ta\tb\tlo\thi\tactual_half_input\tdynamic_clamp\tliteral_clamp_01\tseparate_saturate\tordinary_checks\tmatched_direct_nan_row\tdirect_dynamic\tdirect_literal\tdirect_saturate\n");
    let mut composed_nans = [0usize; 2];
    let mut literal_dynamic_01 = [0usize; 2];
    let mut literal_saturate = [0usize; 2];
    let mut profile_differences = [0usize; 3];
    let mut composition_differences = [[0usize; 3]; 2];
    for (pi, (name, trace)) in [("fast", &fast), ("precise", &precise)]
        .into_iter()
        .enumerate()
    {
        let mut hist: BTreeMap<(u32, usize, usize), Histogram> = BTreeMap::new();
        for (i, row) in c.iter().enumerate() {
            let x = value(trace, i, 0);
            if row.route == 0 && nan(row.a) {
                let kind = usize::from(row.a & 0x8000 != 0) * 2 + usize::from(row.a & 0x0200 != 0);
                for op in 0..3 {
                    let (lo, hi) = if op == 0 {
                        (row.lo, row.hi)
                    } else {
                        (0, 0x3c00)
                    };
                    hist.entry((row.bound, op, kind)).or_default().add(
                        value(trace, i, op + 1),
                        x,
                        lo,
                        hi,
                    );
                }
            } else {
                let verified = (0..3)
                    .filter(|op| expected_control(*row, name, *op).is_some())
                    .count();
                let direct = if row.route == 1 {
                    direct_nan_index(row.bound, x)
                } else {
                    None
                };
                let joined = if let Some(j) = direct {
                    need(
                        c[j].a == x && c[j].bound == row.bound,
                        "direct NaN composition join",
                    )?;
                    for op in 0..3 {
                        composition_differences[pi][op] +=
                            usize::from(value(trace, i, op + 1) != value(trace, j, op + 1));
                    }
                    format!(
                        "{j}\t{:04x}\t{:04x}\t{:04x}",
                        value(trace, j, 1),
                        value(trace, j, 2),
                        value(trace, j, 3)
                    )
                } else {
                    "NA\tNA\tNA\tNA".into()
                };
                controls.push_str(&format!("{name}\t{i}\t{}\t{}\t{:04x}\t{:04x}\t{:04x}\t{:04x}\t{x:04x}\t{:04x}\t{:04x}\t{:04x}\t{verified}\t{joined}\n",row.bound,row.route,row.a,row.b,row.lo,row.hi,value(trace,i,1),value(trace,i,2),value(trace,i,3)));
                composed_nans[pi] += usize::from(row.route == 1 && nan(x));
            }
            if row.bound == 0 {
                literal_dynamic_01[pi] += usize::from(value(trace, i, 1) != value(trace, i, 2));
            }
            literal_saturate[pi] += usize::from(value(trace, i, 2) != value(trace, i, 3));
            if pi == 0 {
                for (op, diff) in profile_differences.iter_mut().enumerate() {
                    *diff += usize::from(value(&fast, i, op + 1) != value(&precise, i, op + 1));
                }
            }
        }
        for ((bound, op, kind), h) in hist {
            let (lo, hi) = if op == 0 {
                BOUNDS[bound as usize]
            } else {
                (0, 0x3c00)
            };
            let words = h
                .counts
                .iter()
                .take(16)
                .map(|(word, count)| format!("{word:04x}:{count}"))
                .collect::<Vec<_>>()
                .join(",");
            nan_table.push_str(&format!("{name}\t{bound}\t{}\t{}\t{}\t{lo:04x}\t{hi:04x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{words}\n",["dynamic_clamp","literal_clamp_01","separate_saturate"][op],["positive_sNaN_bits","positive_qNaN_bits","negative_sNaN_bits","negative_qNaN_bits"][kind],h.n,h.lo,h.hi,h.both,h.input,h.nans,h.infs,h.poszero,h.negzero,h.counts.len()));
        }
    }
    let json=format!(concat!("{{\"status\":\"OBSERVATIONS_COMPLETE\",\"commands\":3,\"cases\":24848,\"raw_half_nan_encodings_per_bound\":2046,\"bound_pairs\":12,",
      "\"ordinary_checks_fast_precise\":{:?},\"computed_input_nan_counts_fast_precise\":{:?},\"literal_vs_dynamic_01_differences_fast_precise\":{:?},\"literal_clamp_vs_saturate_differences_fast_precise\":{:?},\"fast_vs_precise_differences_by_operator\":{:?},\"composition_vs_direct_nan_differences_by_operator_fast_precise\":{:?},",
      "\"direct_input_bits_exact\":true,\"guards_checked\":true,\"missing_row_rejected\":true,\"wrong_expected_rejected\":true,\"nan_endpoint_assumed\":false,\"nan_endpoint_qualified\":false,\"fast_math_nonfinite_guarantee\":false,",
      "\"histogram_samples\":\"first16 output words only; complete raw traces are authoritative\",\"scope\":\"specific runtime/compiler observations, not global clamp/saturate equivalence, IR-mode equivalence, f32 or nonfinite-bound admission\"}}\n"),checks,composed_nans,literal_dynamic_01,literal_saturate,profile_differences,composition_differences);
    Ok(Checked {
        json,
        nans: nan_table,
        controls,
        wrong_expected_rejected,
    })
}

// The following SHA-256/Git binding helpers are reused from scripts/rgba16min.
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
        "inputs.bin",
        "cases.json",
        "binding.txt",
        "source-pins.tsv",
        "baseline.txt",
        "tests.txt",
        "command-exit.json",
        "sdk-enums.tsv",
        "compiler.tsv",
        "receipts.tsv",
        "gpu.json",
        "complete.txt",
        "inputs-after.bin",
        "fast.bin",
        "precise.bin",
        "fast-missing.bin",
        "nan-outcomes.tsv",
        "controls.tsv",
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
    need(count <= 21 && total <= LIMIT, "artifact count/total")
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
fn verify(root: &Path, expected: &str, source: &Path) -> Result<()> {
    let c = acquire(root, expected, source)?;
    for (name, data) in [
        ("validation.json", c.json),
        ("nan-outcomes.tsv", c.nans),
        ("controls.tsv", c.controls),
    ] {
        need(
            read_bounded(&root.join(name))? == data.as_bytes(),
            "validation replay",
        )?;
    }
    need(
        c.wrong_expected_rejected
            && read_bounded(&root.join("negative.json"))? == NEGATIVE.as_bytes(),
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
fn upload_guard(root: &Path, expected: &str, source: &Path) -> Result<()> {
    bounds(root)?;
    binding(root, expected, source)?;
    need(
        read_bounded(&root.join("SHA256SUMS"))? == seal_text(root)?.as_bytes(),
        "final upload seal",
    )?;
    let exit = read_bounded(&root.join("command-exit.json"))?;
    if exit == EXIT_OK.as_bytes() {
        verify(root, expected, source)?;
        println!("SEALED_OBSERVATIONS_NOT_NAN_ENDPOINT_QUALIFICATION");
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
            "check" if a.len() == 5 => {
                let c = acquire(root, &a[3], Path::new(&a[4]))?;
                for (name, data) in [
                    ("validation.json", c.json),
                    ("nan-outcomes.tsv", c.nans),
                    ("controls.tsv", c.controls),
                ] {
                    write_new(&root.join(name), data.as_bytes())?;
                }
            }
            "negative" if a.len() == 5 => {
                let c = acquire(root, &a[3], Path::new(&a[4]))?;
                need(c.wrong_expected_rejected, "wrong expected failed to reject")?;
                write_new(&root.join("negative.json"), NEGATIVE.as_bytes())?;
                println!("WRONG_EXPECTED_REJECTED: unchanged native raw half result");
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
    // In-memory test fixture only. Never exported, never presented as GPU data.
    fn fixture(profile: &str, tag: u32, nan_output: u16) -> Vec<u8> {
        let mut t = vec![POISON; G * 2 + COUNT * RECORD];
        for (i, c) in cases().iter().enumerate() {
            let p = G + i * RECORD;
            for (k, v) in [marker(tag, *c), i as u32].into_iter().enumerate() {
                t[p + k * 4..p + k * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            for (k, v) in [c.a, c.b, c.lo, c.hi].into_iter().enumerate() {
                t[p + 8 + k * 2..p + 10 + k * 2].copy_from_slice(&v.to_le_bytes());
            }
            let x = if c.route == 0 { c.a } else { 0x7e00 };
            let mut v = [x, nan_output, nan_output, nan_output];
            for op in 0..3 {
                if let Some(w) = expected_control(*c, profile, op) {
                    v[op + 1] = w;
                }
            }
            for (k, v) in v.into_iter().enumerate() {
                t[p + 16 + k * 2..p + 18 + k * 2].copy_from_slice(&v.to_le_bytes());
                t[p + 24 + k * 2..p + 26 + k * 2].copy_from_slice(&(!v).to_le_bytes());
            }
        }
        t
    }
    #[test]
    fn all_2046_nan_encodings_per_bound_are_exact_and_complete() {
        let c = cases();
        assert_eq!(c.len(), COUNT);
        for b in 0..12 {
            let slice = &c[b * (NANS + 24)..b * (NANS + 24) + NANS];
            let set: std::collections::BTreeSet<_> = slice.iter().map(|x| x.a).collect();
            assert_eq!(set.len(), 2046);
            assert!(set.iter().all(|x| nan(*x)));
            assert_eq!(set.iter().filter(|x| **x & 0x8000 != 0).count(), 1023);
            assert_eq!(set.iter().filter(|x| **x & 0x0200 != 0).count(), 1024);
        }
        assert_eq!(c[CONTROL_ROW].a, 0x3c00);
    }
    #[test]
    fn only_finite_ordered_half_bounds() {
        for c in cases() {
            assert!(finite(c.lo) && finite(c.hi) && decode(c.lo) <= decode(c.hi));
        }
        assert!(BOUNDS.contains(&(0x8000, 0)));
        assert!(BOUNDS.contains(&(0, 0x8000)));
        assert!(BOUNDS.contains(&(1, 0x03ff)));
        assert!(BOUNDS.contains(&(0x83ff, 0x8001)));
    }
    #[test]
    fn composition_joins_actual_nan_payload_not_a_cpu_endpoint() {
        let c = cases();
        for (i, row) in c
            .iter()
            .enumerate()
            .filter(|(_, c)| c.route == 0 && nan(c.a))
        {
            assert_eq!(direct_nan_index(row.bound, row.a), Some(i));
        }
        assert_eq!(direct_nan_index(0, 0x7c00), None);
        assert_eq!(direct_nan_index(12, 0x7e00), None);
    }
    #[test]
    fn exact_layout_and_bounded_three_command_plan() {
        assert_eq!(inputs(&cases()).len(), 397568);
        assert_eq!(COUNT.div_ceil(WIDTH), 389);
        assert_eq!(allowed().len(), 21);
        assert_eq!(COUNT * RECORD + G * 2, 795264);
        assert!(((3 * (COUNT * RECORD + G * 2) + 2 * COUNT * 16 + G * 2 + 200000) as u64) < LIMIT);
        assert_eq!(plan(), plan());
    }
    #[test]
    fn nan_endpoint_is_never_expected_by_checker() {
        for profile in ["fast", "precise"] {
            for output in [0, 0x3c00, 0x7e00, 0xfc01, 0x7c00, 0x3555] {
                let t = fixture(profile, 1, output);
                validate_capture(&t, profile, 1, &cases()).unwrap();
            }
        }
        for c in cases().into_iter().filter(|c| nan(c.a)) {
            for op in 0..3 {
                assert_eq!(expected_control(c, "fast", op), None);
                assert_eq!(expected_control(c, "precise", op), None);
            }
        }
    }
    #[test]
    fn finite_and_precise_infinity_controls_preserve_existing_contract() {
        let c = Case {
            a: 0x7c00,
            b: 0,
            lo: 0xb800,
            hi: 0x3800,
            route: 0,
            bound: 4,
        };
        assert_eq!(expected_control(c, "precise", 0), Some(0x3800));
        assert_eq!(expected_control(c, "fast", 0), None);
        assert_eq!(
            expected_control(Case { a: 0xfc00, ..c }, "precise", 0),
            Some(0xb800)
        );
        assert_eq!(
            expected_control(Case { a: 0x3400, ..c }, "fast", 0),
            Some(0x3400)
        );
        assert_eq!(expected_control(Case { a: 1, ..c }, "precise", 0), None);
    }
    #[test]
    fn computed_chain_is_observed_not_cpu_assumed_nan() {
        let c = cases();
        assert_eq!(c.iter().filter(|c| c.route == 1).count(), 8);
        for row in c.iter().filter(|c| c.route == 1) {
            for op in 0..3 {
                assert_eq!(expected_control(*row, "fast", op), None);
            }
        }
        let mut t = fixture("fast", 1, 0x3c00);
        for i in COUNT - 8..COUNT {
            t[G + i * RECORD + 16..G + i * RECORD + 18].copy_from_slice(&0x3c00u16.to_le_bytes());
            t[G + i * RECORD + 24..G + i * RECORD + 26]
                .copy_from_slice(&(!0x3c00u16).to_le_bytes());
        }
        validate_capture(&t, "fast", 1, &c).unwrap();
    }
    #[test]
    fn missing_row_negative_is_ordinary_rejection() {
        let mut t = fixture("fast", 3, 0);
        t[G..G + RECORD].fill(POISON);
        assert_eq!(
            validate_capture(&t, "fast", 3, &cases()).unwrap_err(),
            "trace-marker row=0"
        );
    }
    #[test]
    fn unwritten_nan_result_words_cannot_be_accepted_as_poison_outcomes() {
        for slot in 1..4 {
            let mut t = fixture("fast", 1, 0);
            t[G + 16 + slot * 2..G + 18 + slot * 2].fill(POISON);
            t[G + 24 + slot * 2..G + 26 + slot * 2].fill(POISON);
            assert_eq!(
                validate_capture(&t, "fast", 1, &cases()).unwrap_err(),
                format!("result-integrity row=0 slot={slot}")
            );
        }
    }
    #[test]
    fn strict_guards_lengths_source_echo_and_markers() {
        for p in [
            0,
            G - 1,
            G,
            G + 4,
            G + 8,
            G + 12,
            G + 16,
            G + 24,
            G + COUNT * RECORD,
        ] {
            let mut t = fixture("fast", 1, 0);
            t[p] ^= 1;
            assert!(
                validate_capture(&t, "fast", 1, &cases()).is_err(),
                "offset {p}"
            );
        }
        for longer in [true, false] {
            let mut t = fixture("fast", 1, 0);
            if longer {
                t.push(0);
            } else {
                t.pop();
            }
            assert!(validate_capture(&t, "fast", 1, &cases()).is_err());
        }
    }
    #[test]
    fn wrong_expected_bit_never_modifies_actual() {
        let t = fixture("fast", 1, 0);
        let before = sha256(&t);
        expect_word(&t, CONTROL_ROW, 0, 0x3c00).unwrap();
        assert_eq!(
            expect_word(&t, CONTROL_ROW, 0, 0x3c01).unwrap_err(),
            format!("ordinary-value row={CONTROL_ROW} op=0")
        );
        assert_eq!(sha256(&t), before);
    }
    #[test]
    fn math_enum_matching_is_sdk_relative_not_private_selector() {
        let e=enum_check("symbol\tvalue\nrgba8Unorm\t70\ncompleted\t4\nlanguage24\t131076\nload_dontcare\t0\nstore_dontcare\t0\nmode_fast\t7\nmode_safe\t9\nfunctions_fast\t11\nfunctions_precise\t13\n").unwrap();
        let good=format!("{COMPILER_HEADER}\nfast\t1\t7\t11\t131076\t0\t0\t1\t1\t1\t0\t1\t7\t11\t131076\nprecise\t0\t9\t13\t131076\t0\t0\t1\t1\t1\t0\t1\t7\t11\t131076\n");
        compiler_check(&good, &e).unwrap();
        assert!(compiler_check(&good.replacen("fast\t1\t7", "fast\t0\t7", 1), &e).is_err());
        assert!(compiler_check(&good.replace("precise\t0\t9", "precise\t0\t7"), &e).is_err());
    }
    fn receipts() -> String {
        let mut s = format!("{RECEIPT_HEADER}\n");
        for (i, (name, profile)) in [
            ("fast", "fast"),
            ("precise", "precise"),
            ("fast-missing", "fast"),
        ]
        .into_iter()
        .enumerate()
        {
            s.push_str(&format!("{}\t{name}\t{profile}\t4\t0\t0\t0\t70\t64\t389\t1\t15\t0\t1\t0\t0\t0\t0\t1\t3\t1\t24848\t397696\t795264\n",i+1));
        }
        s
    }
    #[test]
    fn receipt_mutations_cannot_pass() {
        let good = receipts();
        receipt_check(&good).unwrap();
        for col in [
            0, 3, 4, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
        ] {
            let mut lines: Vec<_> = good.lines().map(str::to_string).collect();
            let mut f: Vec<_> = lines[1].split('\t').map(str::to_string).collect();
            f[col] = "99".into();
            lines[1] = f.join("\t");
            assert!(receipt_check(&(lines.join("\n") + "\n")).is_err());
        }
        assert!(receipt_check(&(good + "1\textra\n")).is_err());
    }
    #[test]
    fn partial_failure_not_success() {
        assert!(partial_failure_exit(b"{\"exit\":3,\"phase\":\"native\"}\n"));
        assert!(!partial_failure_exit(EXIT_OK.as_bytes()));
        assert!(!partial_failure_exit(
            b"{\"exit\":0,\"phase\":\"native\"}\n"
        ));
    }
    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
