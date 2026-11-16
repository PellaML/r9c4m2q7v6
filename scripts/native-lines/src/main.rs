mod cases;
mod check;
mod ffi;
mod json;

use cases::{CASE_COUNT, CLEAR_ONE, CLEAR_ZERO};
use check::Assertion;
use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Options {
    out: PathBuf,
    registry: u64,
    measure_any: bool,
}
fn options() -> Result<Option<Options>, &'static str> {
    let mut args = std::env::args().skip(1);
    let (mut out, mut registry, mut measure_any) = (None, 0, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" if out.is_none() => out = args.next().map(PathBuf::from),
            "--registry-id" if registry == 0 => {
                let value = args.next().ok_or("missing_registry_id")?;
                registry = u64::from_str_radix(value.strip_prefix("0x").unwrap_or(&value), 16)
                    .map_err(|_| "invalid_registry_id")?;
                if registry == 0 {
                    return Err("zero_registry_id_is_reserved_for_auto_selection");
                }
            }
            "--measure-any-device" if !measure_any => measure_any = true,
            "--help" => {
                println!("generic-native-lines --out NEW_DIRECTORY [--registry-id HEX] [--measure-any-device]\nDefault: measure only a device whose public name contains Apple and Paravirtual.\nOther devices NEVER qualify, including with --measure-any-device.\nNo coverage algorithm is inferred or implemented. No network, capture, or CI dispatch.");
                return Ok(None);
            }
            _ => return Err("invalid_or_duplicate_argument"),
        }
    }
    Ok(Some(Options {
        out: out.ok_or("missing_out_directory")?,
        registry,
        measure_any,
    }))
}
fn write_file(
    path: impl AsRef<std::path::Path>,
    bytes: impl AsRef<[u8]>,
) -> Result<(), &'static str> {
    fs::write(path, bytes).map_err(|_| "write_failed")
}

fn run(o: Options) -> Result<i32, &'static str> {
    // Refuse existing directories/symlinks. Partial results cannot become a later success.
    fs::create_dir(&o.out).map_err(|_| "output_not_fresh_or_unwritable")?;
    write_file(o.out.join("summary.json"),
        "{\"schema\":\"generic-native-lines-summary-v1\",\"status\":\"UNQUALIFIED\",\"reason\":\"incomplete\",\"line_coverage_rule\":\"UNKNOWN\",\"implementation_qualified\":false}\n")?;
    let cases = cases::generate();
    write_file(o.out.join("inputs.json"), json::cases(&cases))?;
    let mut observations =
        File::create(o.out.join("observations.jsonl")).map_err(|_| "create_failed")?;
    let mut readbacks = File::create(o.out.join("readbacks.bin")).map_err(|_| "create_failed")?;
    let start = Instant::now();
    let (native, metadata, init) = ffi::Native::open(o.registry);
    let expected_build = std::env::var("EXPECTED_BUILD").unwrap_or_default();
    if expected_build.len() > 32 || !expected_build.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("invalid_expected_build");
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock_failed")?
        .as_secs();
    let record = format!("{{\"schema\":\"generic-native-lines-run-v1\",\"unix_seconds\":{time},\"native_initialized\":{},\"expected_build\":{},\"require_paravirtual_for_pass\":true,\"measure_any_device_requested\":{},\"metadata\":{},\"initialization\":{},\"abi_sizes\":{},\"readback_layout\":{{\"width\":32,\"height\":32,\"slab_bytes\":{},\"prefix_bytes\":256,\"suffix_bytes\":256,\"row_pitch_bytes\":256,\"guard_and_padding_poison\":165,\"pixel_endianness\":\"little\",\"mode_0_planes\":[\"equal_1_rgba8\",\"equal_0_rgba8\",\"rgba16f_before\",\"rg16f_before\",\"rgba16f_after\",\"rg16f_after\"],\"mode_0_bytes_per_pixel\":[4,4,8,4,8,4],\"mode_1_planes\":[\"equal_1_rgba8\",\"equal_0_rgba8\"],\"mode_1_bytes_per_pixel\":[4,4],\"bitmap_order\":\"rows top to bottom, bits x=0 to x=31 left to right\",\"query\":\"full-screen color pass, Equal 1/0, read mask 255, write mask 0, Keep; RGBA8Unorm black clear and white output\"}}}}\n",
        native.is_some(), json::q(&expected_build), o.measure_any, json::metadata(&metadata),
        json::receipt(&init), json::array(ffi::abi_sizes()), ffi::SLAB);
    write_file(o.out.join("run.json"), record)?;
    let pv = metadata.paravirtual_name_match == 1;
    let build_ok = expected_build.is_empty() || ffi::cstr(&metadata.os_build) == expected_build;
    let mut checks = vec![
        Assertion::simple("native_initialized", native.is_some()),
        Assertion::simple("reported_apple_paravirtual_device", pv),
        Assertion::simple(
            "both_attachment_modes_supported",
            metadata.available_modes == 3,
        ),
        Assertion::simple("expected_os_build_if_requested", build_ok),
    ];
    let mut masks = vec![vec![None; CASE_COUNT]; 2];
    let (mut attempted, mut valid, mut raw_offset) = (0, 0, 0u64);
    let mut stop_reason = if native.is_none() {
        ffi::cstr(&init.reason)
    } else if !pv && !o.measure_any {
        "non_target_device_not_measured".into()
    } else if !build_ok {
        "unexpected_os_build".into()
    } else {
        String::new()
    };
    if let Some(device) = &native {
        if (pv || o.measure_any) && build_ok {
            // Calibrations run first; no geometry rule is used to construct any expected line mask.
            let order: Vec<_> = [CLEAR_ZERO, CLEAR_ONE]
                .into_iter()
                .chain(0..CLEAR_ZERO)
                .collect();
            'modes: for (mode, mode_masks) in masks.iter_mut().enumerate() {
                if metadata.available_modes & (1 << mode) == 0 {
                    continue;
                }
                for &id in &order {
                    if start.elapsed() > Duration::from_secs(90) {
                        stop_reason = "batch_wall_time_limit".into();
                        break 'modes;
                    }
                    attempted += 1;
                    let (receipt, raw) = device.run(&cases[id].input, mode as u32);
                    let mut errors: Vec<&str> = Vec::new();
                    let (mut bitmap, mut count, mut offset, mut length) =
                        ("null".into(), None, None, 0);
                    if let Some(raw) = raw {
                        offset = Some(raw_offset);
                        length = raw.len();
                        readbacks
                            .write_all(&raw)
                            .map_err(|_| "readback_write_failed")?;
                        readbacks.flush().map_err(|_| "readback_flush_failed")?;
                        raw_offset += raw.len() as u64;
                        let decoded = check::inspect(&raw, mode as u32);
                        errors.extend(decoded.errors);
                        if let Some(mask) = decoded.mask {
                            bitmap = json::mask_rows(&mask);
                            count = Some(mask.iter().filter(|&&b| b == 1).count());
                            if errors.is_empty() && receipt.completed(mode as u32) {
                                valid += 1;
                                mode_masks[id] = Some(mask);
                            }
                        }
                    } else {
                        errors.push("no_completed_native_readback");
                    }
                    let ok = errors.is_empty() && receipt.completed(mode as u32);
                    checks.push(Assertion::simple(
                        format!("native_readback_mode_{mode}_case_{id}"),
                        ok,
                    ));
                    let row = format!("{{\"schema\":\"generic-native-lines-observation-v1\",\"case_id\":{id},\"mode\":{mode},\"validated_readback\":{ok},\"receipt\":{},\"raw_offset\":{},\"raw_bytes\":{length},\"covered_pixels\":{},\"bitmap_rows\":{bitmap},\"errors\":{}}}\n",
                        json::receipt(&receipt), json::option(offset), json::option(count),
                        json::array(errors.iter().map(|s| json::q(s))));
                    observations
                        .write_all(row.as_bytes())
                        .map_err(|_| "observation_write_failed")?;
                    observations
                        .flush()
                        .map_err(|_| "observation_flush_failed")?;
                    if !receipt.completed(mode as u32) {
                        stop_reason = ffi::cstr(&receipt.reason);
                        if stop_reason.is_empty() {
                            stop_reason = "native_failure".into();
                        }
                        break 'modes;
                    }
                }
            }
        }
    }
    checks.push(Assertion::simple(
        "all_132_native_measurements_valid",
        valid == 2 * CASE_COUNT,
    ));
    checks.extend(check::relations(&cases, &masks));
    let qualified = check::qualifies(native.is_some(), pv, valid, &checks);
    if stop_reason.is_empty() && !qualified {
        stop_reason = "qualification_checks_failed".into();
    }
    let status = if qualified {
        "QUALIFIED_NATIVE_MEASUREMENT"
    } else {
        "UNQUALIFIED"
    };
    let summary = format!("{{\"schema\":\"generic-native-lines-summary-v1\",\"status\":{},\"reason\":{},\"line_coverage_rule\":\"UNKNOWN\",\"implementation_qualified\":false,\"scope\":\"Only these emitted native samples on the recorded device/build; no line rasterization algorithm inferred\",\"expected_cases\":{CASE_COUNT},\"expected_measurements\":132,\"attempted_measurements\":{attempted},\"valid_measurements\":{valid},\"raw_readback_bytes\":{raw_offset},\"elapsed_seconds\":{},\"failed_checks\":{},\"checks\":{}}}\n",
        json::q(status), json::q(&stop_reason), start.elapsed().as_secs_f64(),
        checks.iter().filter(|c| !c.passed).count(), json::array(checks.iter().map(json::assertion)));
    write_file(o.out.join("summary.json"), summary)?;
    println!("{status}: {valid}/132 valid native observations; line coverage rule UNKNOWN");
    Ok(if qualified { 0 } else { 2 })
}

fn main() -> std::process::ExitCode {
    let result = options().and_then(|o| o.map_or(Ok(0), run));
    match result {
        Ok(code) => std::process::ExitCode::from(code as u8),
        Err(reason) => {
            eprintln!("UNQUALIFIED: {reason}");
            std::process::ExitCode::from(2)
        }
    }
}
