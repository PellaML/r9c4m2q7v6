//! Generic ALU oracle. No texture conversion and no CPU execution fallback.
use std::{env, fs, path::Path};
type Result<T> = std::result::Result<T, &'static str>;
const COUNT: usize = 1040;
const STAMP: u32 = 0x51c0_ffee;
const ARTIFACTS: [&str; 10] = [
    "inputs.bin",
    "expected-half.bin",
    "samples.bin",
    "native.json",
    "check.json",
    "wrong-expected.json",
    "manifest.json",
    "toolchain.txt",
    "unpack.air",
    "unpack.metallib",
];

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    word: u32,
    index: u32,
    bits: [u16; 4],
    stamp: u32,
    reserved: u32,
}
#[cfg(any(test, unpack_native))]
impl Sample {
    fn poison() -> Self {
        Self {
            word: u32::MAX,
            index: u32::MAX,
            bits: [0x7e00; 4],
            stamp: 0,
            reserved: u32::MAX,
        }
    }
}

fn words() -> Vec<u32> {
    let mut words = Vec::with_capacity(COUNT);
    for lane in 0..4 {
        for byte in 0..=255u32 {
            let shift = lane * 8;
            words.push((0xdf9b_5713 & !(255 << shift)) | (byte << shift));
        }
    }
    words.extend([
        0,
        0xffff_ffff,
        0x0001_0203,
        0x0302_0100,
        0x0103_07ff,
        0xff07_0301,
        0x7f80_fe01,
        0x01fe_807f,
        0x55aa_00ff,
        0xff00_aa55,
        0x8080_8080,
        0x7f7f_7f7f,
        0x0001_0000,
        0x0100_0000,
        0x0000_0100,
        0x0000_0001,
    ]);
    words
}

// Exact rational byte/255, rounded to binary16, ties-to-even. No host float
// approximation or texture sampling oracle is reused. Every nonzero value is
// normal, so the bounded numerator needs at most 18 shift bits.
fn normalized_half(byte: u8) -> u16 {
    if byte == 0 {
        return 0;
    }
    let n = u32::from(byte);
    let mut shift = 0;
    while (n << shift) < 255 {
        shift += 1;
    }
    let numerator = n << (10 + shift);
    let mut significand = numerator / 255;
    let remainder = numerator % 255;
    if remainder * 2 > 255 || (remainder * 2 == 255 && significand & 1 != 0) {
        significand += 1;
    }
    (((15 - shift) << 10) + significand - 1024) as u16
}
fn expected(word: u32) -> [u16; 4] {
    std::array::from_fn(|lane| normalized_half((word >> (lane * 8)) as u8))
}
fn input_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn expected_bytes(words: &[u32]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|w| expected(*w))
        .flat_map(u16::to_le_bytes)
        .collect()
}
#[cfg(any(test, unpack_native))]
fn sample_bytes(samples: &[Sample]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 24);
    for sample in samples {
        bytes.extend(sample.word.to_le_bytes());
        bytes.extend(sample.index.to_le_bytes());
        bytes.extend(sample.bits.into_iter().flat_map(u16::to_le_bytes));
        bytes.extend(sample.stamp.to_le_bytes());
        bytes.extend(sample.reserved.to_le_bytes());
    }
    bytes
}
fn decode_samples(bytes: &[u8]) -> Result<Vec<Sample>> {
    if bytes.len() != COUNT * 24 {
        return Err("wrong sample extent");
    }
    Ok(bytes
        .chunks_exact(24)
        .map(|b| Sample {
            word: u32::from_le_bytes(b[0..4].try_into().unwrap()),
            index: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            bits: std::array::from_fn(|lane| {
                u16::from_le_bytes(b[8 + lane * 2..10 + lane * 2].try_into().unwrap())
            }),
            stamp: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            reserved: u32::from_le_bytes(b[20..24].try_into().unwrap()),
        })
        .collect())
}
#[derive(Debug, PartialEq, Eq)]
struct Check {
    numeric_mismatches: usize,
    identity_errors: usize,
}
fn examine(words: &[u32], samples: &[Sample], wrong: bool) -> Result<Check> {
    if words.len() != COUNT || samples.len() != COUNT {
        return Err("wrong case count");
    }
    let mut check = Check {
        numeric_mismatches: 0,
        identity_errors: 0,
    };
    for (index, (word, sample)) in words.iter().zip(samples).enumerate() {
        if sample.word != *word
            || sample.index != index as u32
            || sample.stamp != (word ^ index as u32 ^ STAMP)
            || sample.reserved != 0
        {
            check.identity_errors += 1;
        }
        let mut want = expected(*word);
        // Deliberately wrong expected cell: mutate the oracle, not the readback.
        if wrong && index == 0 {
            want[0] ^= 1;
        }
        check.numeric_mismatches += want
            .into_iter()
            .zip(sample.bits)
            .filter(|(a, b)| a != b)
            .count();
    }
    Ok(check)
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|_| "missing artifact")?;
    if !meta.is_file() || meta.len() > limit {
        return Err("artifact extent or type");
    }
    fs::read(path).map_err(|_| "artifact read failed")
}
fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, bytes).map_err(|_| "artifact write failed")
}
fn check_files(out: &Path, wrong: bool) -> Result<i32> {
    let words = words();
    if read_bounded(&out.join("inputs.bin"), 4160)? != input_bytes(&words)
        || read_bounded(&out.join("expected-half.bin"), 8320)? != expected_bytes(&words)
    {
        return Err("input or expected table is not canonical");
    }
    let samples = decode_samples(&read_bounded(&out.join("samples.bin"), 24960)?)?;
    let result = examine(&words, &samples, wrong)?;
    let passed = result.numeric_mismatches == 0 && result.identity_errors == 0;
    let negative_ok = wrong && result.numeric_mismatches == 1 && result.identity_errors == 0;
    let json = format!("{{\"cases\":1040,\"half_cells\":4160,\"numeric_mismatches\":{},\"identity_errors\":{},\"passed\":{},\"deliberately_wrong_expected\":{},\"wrong_expected_cell\":{},\"exact_single_mismatch_control\":{}}}\n",
        result.numeric_mismatches, result.identity_errors, passed, wrong,
        if wrong { "0" } else { "null" }, negative_ok);
    write(
        &out.join(if wrong {
            "wrong-expected.json"
        } else {
            "check.json"
        }),
        json,
    )?;
    // Exit1 for the negative mode means the exact deliberate mismatch, never IO,
    // missing hardware, malformed records, stale readback or unrelated mismatch.
    Ok(if wrong {
        if negative_ok {
            1
        } else {
            3
        }
    } else if passed {
        0
    } else {
        1
    })
}

#[cfg(unpack_native)]
unsafe extern "C" {
    fn unpack_run(
        library: *const std::ffi::c_char,
        words: *const u32,
        count: u32,
        samples: *mut Sample,
        info: *mut u32,
        json: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
}
#[cfg(unpack_native)]
fn run(library: &Path, out: &Path, fast: bool) -> Result<i32> {
    use std::ffi::CString;
    let library = CString::new(library.to_str().ok_or("library path encoding")?)
        .map_err(|_| "library path contains NUL")?;
    fs::create_dir(out).map_err(|_| "new output directory required")?;
    let words = words();
    write(&out.join("inputs.bin"), input_bytes(&words))?;
    write(&out.join("expected-half.bin"), expected_bytes(&words))?;
    let mut samples = vec![Sample::poison(); COUNT];
    let mut info = [0u32; 4];
    let mut metadata = [0u8; 4096];
    // C owns no borrowed memory after return. All spans match the fixed cohort;
    // output is copied only after successful native command completion.
    let status = unsafe {
        unpack_run(
            library.as_ptr(),
            words.as_ptr(),
            COUNT as u32,
            samples.as_mut_ptr(),
            info.as_mut_ptr(),
            metadata.as_mut_ptr().cast(),
            metadata.len(),
        )
    };
    let end = metadata
        .iter()
        .position(|b| *b == 0)
        .ok_or("unterminated native metadata")?;
    let native = std::str::from_utf8(&metadata[..end]).map_err(|_| "invalid native metadata")?;
    if !native.starts_with('{') || !native.ends_with('}') {
        return Err("missing native metadata");
    }
    write(&out.join("native.json"), format!("{native}\n"))?;
    if status != 0 {
        return Ok(status);
    }
    if info != [1, 1, 4, 1] {
        return Err("native pipeline/wait/completion/readback join failed");
    }
    write(&out.join("samples.bin"), sample_bytes(&samples))?;
    let status = check_files(out, false)?;
    write(&out.join("manifest.json"), format!(concat!(
        "{{\"schema\":1,\"operation\":\"unpack_unorm4x8_to_half\",",
        "\"availability\":\"available_and_executed\",\"stage\":\"compute\",",
        "\"standard\":\"metal4.0\",\"optimization\":\"{}\",",
        "\"deployment\":\"compiler default\",\"cases\":1040,\"half_cells\":4160,",
        "\"sample_stride\":24,\"byte_order\":\"little endian\",",
        "\"expected\":\"exact rational byte/255, binary16 ties-to-even; low byte is lane0\",",
        "\"numeric_passed\":{},\"air_signature_validation\":\"pending_raw_artifact_inspection\"}}\n"),
        if fast { "-O2 -g0 -ffast-math" } else { "-O2 -g0 -fno-fast-math" }, status == 0))?;
    Ok(status)
}
#[cfg(not(unpack_native))]
fn run(_: &Path, _: &Path, _: bool) -> Result<i32> {
    Err("native bridge not linked; no CPU fallback")
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && bytes.windows(needle.len()).any(|part| part == needle)
}
// Same exact public-hosted checkout exception as the existing public probes.
// A local, self-hosted, mismatched or unrelated path is not made publishable.
fn public_workspace_paths(
    root: &Path,
    out: &Path,
    home: &str,
    workspace: &str,
    repository: &str,
) -> bool {
    let Some((owner, name)) = repository.split_once('/') else {
        return false;
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') || home != "/Users/runner" {
        return false;
    }
    let expected = Path::new("/Users/runner/work").join(name).join(name);
    root == expected && Path::new(workspace) == expected && out.starts_with(&expected)
}
fn audit(out: &Path) -> Result<i32> {
    // Fail closed before the shell copies any files to the artifact allowlist.
    let mut paths = vec![env::current_dir()
        .map_err(|_| "working directory unavailable")?
        .to_string_lossy()
        .into_owned()];
    if let Ok(home) = env::var("HOME") {
        paths.push(home);
    }
    let root = env::current_dir().map_err(|_| "working directory unavailable")?;
    let actual_out = out
        .canonicalize()
        .map_err(|_| "artifact directory unavailable")?;
    if env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
        && public_workspace_paths(
            &root,
            &actual_out,
            &env::var("HOME").unwrap_or_default(),
            &env::var("GITHUB_WORKSPACE").unwrap_or_default(),
            &env::var("GITHUB_REPOSITORY").unwrap_or_default(),
        )
    {
        paths.clear();
    }
    for name in ARTIFACTS {
        let bytes = read_bounded(&out.join(name), 4 * 1024 * 1024)?;
        if bytes.is_empty() {
            return Err("empty artifact");
        }
        for path in &paths {
            if path.len() <= 1 {
                return Err("invalid privacy path");
            }
            let utf16le: Vec<_> = path.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let utf16be: Vec<_> = path.encode_utf16().flat_map(u16::to_be_bytes).collect();
            if contains(&bytes, path.as_bytes())
                || contains(&bytes, &utf16le)
                || contains(&bytes, &utf16be)
            {
                return Err("local path in artifact; publication refused");
            }
        }
    }
    Ok(0)
}
fn main() {
    let args: Vec<_> = env::args_os().collect();
    let result = match args.get(1).and_then(|arg| arg.to_str()) {
        Some("run") if args.len() == 5 => match args[4].to_str() {
            Some("strict") => run(Path::new(&args[2]), Path::new(&args[3]), false),
            Some("fast") => run(Path::new(&args[2]), Path::new(&args[3]), true),
            _ => Err("math mode must be strict or fast"),
        },
        Some("check") if args.len() == 3 => check_files(Path::new(&args[2]), false),
        Some("wrong-expected") if args.len() == 3 => check_files(Path::new(&args[2]), true),
        Some("audit") if args.len() == 3 => audit(Path::new(&args[2])),
        _ => {
            Err("usage: unpack-oracle run LIB NEW_OUT strict|fast | check OUT | wrong-expected OUT | audit OUT")
        }
    };
    let code = match result {
        Ok(code) => code,
        Err(phase) => {
            eprintln!("unpack probe refused: {phase}");
            3
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_public_hosted_workspace_paths_are_allowed() {
        let root = Path::new("/Users/runner/work/generic/generic");
        assert!(public_workspace_paths(
            root,
            &root.join("out"),
            "/Users/runner",
            root.to_str().unwrap(),
            "owner/generic"
        ));
        assert!(!public_workspace_paths(
            root,
            &root.join("out"),
            "/Users/person",
            root.to_str().unwrap(),
            "owner/generic"
        ));
        assert!(!public_workspace_paths(
            root,
            Path::new("/Users/runner/private"),
            "/Users/runner",
            root.to_str().unwrap(),
            "owner/generic"
        ));
        assert!(!public_workspace_paths(
            root,
            &root.join("out"),
            "/Users/runner",
            root.to_str().unwrap(),
            "owner/different"
        ));
        assert!(!public_workspace_paths(
            Path::new("/Users/person/work/generic/generic"),
            &root.join("out"),
            "/Users/runner",
            root.to_str().unwrap(),
            "owner/generic"
        ));
    }
    fn ideal() -> Vec<Sample> {
        words()
            .into_iter()
            .enumerate()
            .map(|(i, word)| Sample {
                word,
                index: i as u32,
                bits: expected(word),
                stamp: word ^ i as u32 ^ STAMP,
                reserved: 0,
            })
            .collect()
    }
    #[test]
    fn every_byte_in_each_lane_and_mixed_words_are_present() {
        let w = words();
        assert_eq!(w.len(), COUNT);
        for lane in 0..4 {
            for byte in 0..=255usize {
                assert_eq!((w[lane * 256 + byte] >> (lane * 8)) & 255, byte as u32);
            }
        }
        assert!(w[1024..].contains(&0x0302_0100));
        assert!(w[1024..].contains(&0x0001_0203));
    }
    #[test]
    fn exact_rational_matches_independent_half_lattice_search() {
        // Compare all values against an integer search of the entire positive
        // binary16 [0,1] lattice, scaled by2^24 (including every subnormal).
        for byte in 0..=255u8 {
            let target = u64::from(byte) << 24;
            let best = (0..=0x3c00u16)
                .min_by_key(|bits| {
                    let exponent = u32::from(bits >> 10);
                    let fraction = u64::from(bits & 1023);
                    let scaled = if exponent == 0 {
                        fraction
                    } else {
                        (1024 + fraction) << (exponent - 1)
                    };
                    (target.abs_diff(scaled * 255), bits & 1)
                })
                .unwrap();
            assert_eq!(normalized_half(byte), best, "byte={byte}");
        }
        assert_eq!(normalized_half(0), 0);
        assert_eq!(normalized_half(1), 0x1c04);
        assert_eq!(normalized_half(128), 0x3804);
        assert_eq!(normalized_half(255), 0x3c00);
    }
    #[test]
    fn ffi_and_file_layouts_are_explicit() {
        assert_eq!(std::mem::size_of::<Sample>(), 24);
        assert_eq!(std::mem::offset_of!(Sample, bits), 8);
        assert_eq!(std::mem::offset_of!(Sample, stamp), 16);
        let data = ideal();
        assert_eq!(decode_samples(&sample_bytes(&data)).unwrap(), data);
        assert!(decode_samples(&[0; 23]).is_err());
    }
    #[test]
    fn wrong_expected_control_changes_exactly_one_cell_not_native_bytes() {
        let data = ideal();
        let before = sample_bytes(&data);
        assert_eq!(
            examine(&words(), &data, false).unwrap(),
            Check {
                numeric_mismatches: 0,
                identity_errors: 0
            }
        );
        assert_eq!(
            examine(&words(), &data, true).unwrap(),
            Check {
                numeric_mismatches: 1,
                identity_errors: 0
            }
        );
        assert_eq!(sample_bytes(&data), before);
    }
    #[test]
    fn wrong_lane_order_poison_and_stale_stamps_are_rejected() {
        let mut data = ideal();
        data[0].bits.reverse();
        assert!(examine(&words(), &data, false).unwrap().numeric_mismatches > 0);
        assert_eq!(
            examine(&words(), &vec![Sample::poison(); COUNT], false)
                .unwrap()
                .identity_errors,
            COUNT
        );
        let mut data = ideal();
        data[0].stamp ^= 1;
        assert_eq!(examine(&words(), &data, false).unwrap().identity_errors, 1);
    }
    #[test]
    fn byte_table_and_expected_lengths_are_fixed() {
        assert_eq!(input_bytes(&words()).len(), 4160);
        assert_eq!(expected_bytes(&words()).len(), 8320);
        assert_eq!(sample_bytes(&ideal()).len(), 24960);
        assert_eq!(expected(0xff80_0100), [0, 0x1c04, 0x3804, 0x3c00]);
    }
    #[test]
    fn shader_calls_only_the_requested_unpack_and_preserves_raw_bits() {
        let shader = include_str!("../../shaders/unpack.metal");
        assert_eq!(shader.matches("unpack_unorm4x8_to_half(").count(), 1);
        assert!(shader.contains("as_type<ushort4>"));
        assert!(!shader.contains("texture"));
        assert!(!shader.contains("/ 255"));
    }

    #[test]
    fn file_checks_distinguish_the_deliberate_mismatch_from_invalid_data() {
        // A fresh bounded child of the caller's test working directory. Never
        // writes native.json or claims these synthetic records came from a GPU.
        let out = env::current_dir()
            .unwrap()
            .join(format!(".unpack-test-{}", std::process::id()));
        fs::create_dir(&out).unwrap();
        write(&out.join("inputs.bin"), input_bytes(&words())).unwrap();
        write(&out.join("expected-half.bin"), expected_bytes(&words())).unwrap();
        let bytes = sample_bytes(&ideal());
        write(&out.join("samples.bin"), &bytes).unwrap();
        assert_eq!(check_files(&out, false), Ok(0));
        assert_eq!(check_files(&out, true), Ok(1));
        assert_eq!(fs::read(out.join("samples.bin")).unwrap(), bytes);
        write(
            &out.join("samples.bin"),
            sample_bytes(&vec![Sample::poison(); COUNT]),
        )
        .unwrap();
        assert_eq!(check_files(&out, true), Ok(3));
        write(&out.join("samples.bin"), [0; 23]).unwrap();
        assert!(check_files(&out, true).is_err());
        for name in [
            "inputs.bin",
            "expected-half.bin",
            "samples.bin",
            "check.json",
            "wrong-expected.json",
        ] {
            fs::remove_file(out.join(name)).unwrap();
        }
        fs::remove_dir(out).unwrap();
    }
}
