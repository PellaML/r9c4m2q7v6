#[cfg_attr(not(any(target_os = "macos", probe_bridge_check)), allow(dead_code))]
mod artifacts;
#[cfg_attr(not(any(target_os = "macos", probe_bridge_check)), allow(dead_code))]
mod binary;
#[cfg_attr(not(any(target_os = "macos", probe_bridge_check)), allow(dead_code))]
mod checks;
mod cohort;
mod json;
#[cfg(any(target_os = "macos", probe_bridge_check, test))]
#[cfg_attr(not(any(target_os = "macos", probe_bridge_check)), allow(dead_code))]
mod native;
#[cfg_attr(not(any(target_os = "macos", probe_bridge_check)), allow(dead_code))]
mod plan;

use json::Json;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
include!(concat!(env!("OUT_DIR"), "/build_info.rs"));

struct Report {
    status: &'static str,
    support: &'static str,
    native: Json,
    shader_status: &'static str,
    toolchain: Json,
    artifacts: Vec<Json>,
    cases: Vec<Json>,
    recorded_cases: usize,
    sample_records: u32,
    required_completed: usize,
    required_missing: usize,
    optional_skipped: usize,
    failed_checks: usize,
    checks_passed: Option<bool>,
    invalid_control: Json,
}
impl Report {
    fn new() -> Self {
        Self {
            status: "initializing",
            support: "not_tested",
            native: Json::Null,
            shader_status: "not_attempted",
            toolchain: Json::Null,
            artifacts: Vec::new(),
            cases: Vec::new(),
            recorded_cases: 0,
            sample_records: 0,
            required_completed: 0,
            required_missing: 0,
            optional_skipped: 0,
            failed_checks: 0,
            checks_passed: None,
            invalid_control: Json::Null,
        }
    }
    fn manifest(&self, cohort: &cohort::Cohort, cases: &[plan::Case]) -> Json {
        Json::object([
            ("schema", Json::from("volume-sample-probe/1")),
            ("status", Json::from(self.status)),
            ("native_gpu_support", Json::from(self.support)),
            ("native_execution", Json::from(self.recorded_cases != 0)),
            ("cpu_sampling_fallback_used", Json::from(false)),
            ("qualification", Json::from("independent_comparison_pending")),
            ("host_os", Json::from(std::env::consts::OS)),
            ("process_architecture", Json::from(std::env::consts::ARCH)),
            ("host_build", Json::Raw(BUILD_INFO.to_string())),
            ("native", self.native.clone()),
            ("toolchain", self.toolchain.clone()),
            ("texture_request", Json::object([
                ("type", Json::from("MTLTextureType3D")),
                ("format", Json::from("BGRA8Unorm")),
                ("extent", Json::Array(vec![1_u32.into(), 1_u32.into(), 1_u32.into()])),
                ("mip_levels", Json::from(1_u32)),
                ("srgb", Json::from(false)),
                ("upload_order", Json::from("BGRA")),
                ("result_order", Json::from("RGBA")),
            ])),
            ("colors", Json::Array(plan::COLORS.iter().enumerate().map(|(i, c)| {
                Json::object([
                    ("id", Json::from(i)),
                    ("rgba_u8", Json::Array(c.iter().map(|v| Json::from(*v as u32)).collect())),
                    ("uploaded_bgra_u8", Json::Array([c[2], c[1], c[0], c[3]].iter().map(|v| Json::from(*v as u32)).collect())),
                ])
            }).collect())),
            ("coordinates", Json::object([
                ("file", Json::from("coordinates.bin")),
                ("count", Json::from(cohort.coordinates.len())),
                ("core_prefix_count", Json::from(cohort.core_count)),
                ("stride_bytes", Json::from(12_u32)),
                ("header_bytes", Json::from(16_u32)),
                ("center_index", Json::from(0_u32)),
                ("center_f32_bits", checks::bits_json(&cohort::CENTER)),
                ("grid_denominator", Json::from(512_u32)),
                ("grid_numerator_min", Json::from(-256_i32)),
                ("grid_numerator_max", Json::from(768_i32)),
                ("odd_numerator_neighbors", Json::from("both adjacent binary32 values")),
                ("sweeps", Json::from("S-only, T-only, R-only, and S=T=R")),
                ("tensor_grid_numerators", Json::Array(cohort::GRID_WEIGHTS.iter().map(|n| Json::from(*n)).collect())),
                ("finite_far_max_bits", checks::bits_json(&[f32::MAX.to_bits(), (-f32::MAX).to_bits()])),
            ])),
            ("sampler_requests", sampler_metadata()),
            ("plan", Json::object([
                ("case_count", Json::from(cases.len())),
                ("required_case_count", Json::from(cases.iter().filter(|c| c.required()).count())),
                ("dense_case_count", Json::from(cases.iter().filter(|c| c.dense).count())),
                ("maximum_sample_table_bytes", Json::from(16 + cases.iter().map(|c| c.count * 20).sum::<usize>())),
                ("dense_scope", Json::from("normalized level0 float/half, linear, runtime/constexpr, masks 0/7/4/3, all colors")),
                ("core_scope", Json::from("remaining level0 cases; implicit/bias runtime normalized nearest/linear masks 0/7/4/3")),
                ("pixel_coordinate_note", Json::from("optional; same numeric coordinates have the same scale for a unit extent; not a general non-unit normalization test")),
                ("implicit_derivative_note", Json::from("fragment raster derivatives over the supplied coordinate buffer; one mip; not a LOD or derivative qualification")),
                ("bias_f32_bits", checks::bits_json(&[0x3f40_0000])),
                ("raster_width", Json::from(256_u32)),
            ])),
            ("shader_artifacts_status", Json::from(self.shader_status)),
            ("shader_artifacts", Json::Array(self.artifacts.clone())),
            ("sample_table", Json::object([
                ("file", if self.recorded_cases == 0 { Json::Null } else { Json::from("samples.bin") }),
                ("record_count", Json::from(self.sample_records)),
                ("stride_bytes", Json::from(20_u32)),
                ("header_bytes", Json::from(16_u32)),
                ("fields", Json::from("RGBA binary32 bit patterns u32[4], completion stamp u32; all little-endian")),
                ("half_note", Json::from("texture3d<half> result converted to float4 in shader, then bitcast to uint4; no decimal round-trip")),
                ("stamp_formula", Json::from("(0xc0010000 XOR case_id) XOR coordinate_index XOR 0x9e3779b9")),
            ])),
            ("summary", Json::object([
                ("recorded_cases", Json::from(self.recorded_cases)),
                ("required_completed", Json::from(self.required_completed)),
                ("required_missing", Json::from(self.required_missing)),
                ("optional_skipped", Json::from(self.optional_skipped)),
                ("self_check_failures", Json::from(self.failed_checks)),
                ("self_checks_passed", self.checks_passed.map(Json::from).unwrap_or(Json::Null)),
                ("self_check_scope", Json::from("successful actual GPU wait, all per-sample stamps, poison overwritten, fixed center black/white/transparent anchors, positive colored centers, wrong oracle rejection")),
                ("nonfinite_far_values", Json::from("reported as raw observations, not given a CPU filtering expectation")),
            ])),
            ("invalid_control", self.invalid_control.clone()),
            ("case_results", Json::Array(self.cases.clone())),
        ])
    }
    fn write(&self, out: &Path, cohort: &cohort::Cohort, cases: &[plan::Case]) -> io::Result<()> {
        fs::write(
            out.join("manifest.json"),
            self.manifest(cohort, cases).encode(),
        )
    }
}

fn sampler_metadata() -> Json {
    Json::Array(
        (0_u32..64)
            .map(|id| {
                let is_constexpr = id & 32 != 0;
                Json::object([
                    ("id", Json::from(id)),
                    (
                        "implementation",
                        Json::from(if is_constexpr { "constexpr" } else { "runtime" }),
                    ),
                    (
                        "coordinates",
                        Json::from(if id & 16 != 0 { "pixel" } else { "normalized" }),
                    ),
                    (
                        "min_mag_filter",
                        Json::from(if id & 8 != 0 { "linear" } else { "nearest" }),
                    ),
                    ("zero_axes_mask", Json::from(id & 7)),
                    (
                        "s",
                        Json::from(if id & 1 != 0 {
                            "clamp_to_zero"
                        } else {
                            "clamp_to_edge"
                        }),
                    ),
                    (
                        "t",
                        Json::from(if id & 2 != 0 {
                            "clamp_to_zero"
                        } else {
                            "clamp_to_edge"
                        }),
                    ),
                    (
                        "r",
                        Json::from(if id & 4 != 0 {
                            "clamp_to_zero"
                        } else {
                            "clamp_to_edge"
                        }),
                    ),
                    ("mip_filter", Json::from("none")),
                    ("max_anisotropy", Json::from(1_u32)),
                    (
                        "lod_clamp",
                        if is_constexpr {
                            Json::from("SDK constexpr defaults; single mip")
                        } else {
                            checks::bits_json(&[0, 0x7f7f_ffff])
                        },
                    ),
                ])
            })
            .collect(),
    )
}

#[cfg(any(target_os = "macos", probe_bridge_check))]
fn case_result(
    case: &plan::Case,
    status: &'static str,
    offset: Option<u32>,
    wait: Json,
    detail: Json,
    checked: Json,
) -> Json {
    Json::object([
        ("id", Json::from(case.id)),
        ("module", Json::from(case.module_name())),
        ("sampler_id", Json::from(case.sampler_id())),
        ("color_id", Json::from(case.color)),
        ("required", Json::from(case.required())),
        (
            "cohort",
            Json::from(if case.dense { "dense" } else { "core" }),
        ),
        ("coordinate_count", Json::from(case.count)),
        ("coordinate_offset", Json::from(0_u32)),
        (
            "sample_offset",
            offset.map(Json::from).unwrap_or(Json::Null),
        ),
        (
            "recorded_count",
            Json::from(if offset.is_some() { case.count } else { 0 }),
        ),
        ("token", checks::bits_json(&[case.token()])),
        ("status", Json::from(status)),
        ("wait", wait),
        ("detail", detail),
        ("checks", checked),
    ])
}

#[cfg(any(target_os = "macos", probe_bridge_check))]
fn run_native(
    root: &Path,
    out: &Path,
    cohort: &cohort::Cohort,
    plan: &[plan::Case],
    report: &mut Report,
) -> io::Result<i32> {
    let (native, open_code, native_info) = native::Native::open();
    report.native = native_info;
    report.support = if open_code == 2 {
        "unavailable"
    } else if open_code == 0 {
        "resources_created_unverified"
    } else {
        "failed"
    };
    report.status = "building_owned_shaders";
    report.write(out, cohort, plan)?;

    // AIR compilation is independent of GPU availability; keep useful owned AIR even on GPU-less hosts.
    let (artifacts, versions) = artifacts::compile(root, out)?;
    report.toolchain = versions;
    report.artifacts = artifacts.iter().map(|a| a.report.clone()).collect();
    let required_shaders_ready = artifacts
        .iter()
        .filter(|a| a.required)
        .all(|a| a.library.is_some());
    report.shader_status = if required_shaders_ready {
        if artifacts.iter().all(|a| a.library.is_some()) {
            "ready"
        } else {
            "required_ready_optional_unavailable"
        }
    } else {
        "required_artifacts_missing"
    };
    if open_code != 0 || native.is_none() {
        report.status = if open_code == 2 {
            "native_unavailable"
        } else {
            "native_open_failed"
        };
        report.required_missing = plan.iter().filter(|c| c.required()).count();
        report.optional_skipped = plan.iter().filter(|c| !c.required()).count();
        report.write(out, cohort, plan)?;
        return Ok(if open_code == 2 { 2 } else { 1 });
    }

    let mut native = native.expect("checked native handle");
    let mut table = binary::Samples::new(&out.join("samples.bin"))?;
    let mut anchors: [Option<binary::Anchor>; 2] = [None, None];
    report.status = "recording";
    for case in plan {
        let module = case.module_name();
        let library = artifacts
            .iter()
            .find(|a| a.name == module)
            .and_then(|a| a.library.as_ref());
        if let Some(library) = library {
            let mut samples = vec![checks::Sample::poison(); case.count];
            let (code, wait, detail) =
                native.sample(library, case, &cohort.coordinates, &mut samples);
            if code == 0 && wait.completed() {
                let offset = table.append(&samples)?;
                let checked = checks::examine(case, &samples);
                report.recorded_cases += 1;
                report.sample_records = table.count;
                report.required_completed += usize::from(case.required());
                report.failed_checks += usize::from(!checked.valid);
                report.support = "available_and_executed";
                if case.anchor() && checked.valid {
                    anchors[case.color] = Some(binary::Anchor::from_case(case, samples[0]));
                }
                report.cases.push(case_result(
                    case,
                    if checked.valid {
                        "recorded"
                    } else {
                        "self_check_failed"
                    },
                    Some(offset),
                    wait.json(),
                    detail,
                    checked.json(),
                ));
            } else {
                report.required_missing += usize::from(case.required());
                report.optional_skipped += usize::from(!case.required());
                report.cases.push(case_result(
                    case,
                    if code == 2 {
                        "native_resource_unavailable"
                    } else {
                        "native_call_failed"
                    },
                    None,
                    wait.json(),
                    detail,
                    Json::Null,
                ));
            }
        } else {
            report.required_missing += usize::from(case.required());
            report.optional_skipped += usize::from(!case.required());
            report.cases.push(case_result(
                case,
                "shader_artifact_unavailable",
                None,
                Json::Null,
                Json::Null,
                Json::Null,
            ));
        }
        if report.cases.len() % 32 == 0 {
            report.write(out, cohort, plan)?;
        }
    }
    report.sample_records = table.finish()?;
    let mut invalid_rejected = false;
    if let [Some(black), Some(white)] = anchors {
        binary::write_anchors(&out.join("anchors.bin"), &[black, white])?;
        // The independent negative control consumes the serialized native observation.
        let observed = binary::read_white_anchor(&out.join("anchors.bin"))?;
        let (code, control) = checks::wrong_oracle_report(observed);
        invalid_rejected = code == 1;
        report.invalid_control = control;
    }
    let required_count = plan.iter().filter(|c| c.required()).count();
    let passed = required_shaders_ready
        && report.required_completed == required_count
        && report.required_missing == 0
        && report.failed_checks == 0
        && invalid_rejected;
    report.checks_passed = Some(passed);
    report.status = if !passed {
        "self_check_or_required_coverage_failed"
    } else if report.optional_skipped != 0 {
        "recorded_with_optional_skips"
    } else {
        "recorded"
    };
    report.write(out, cohort, plan)?;
    Ok(if passed { 0 } else { 1 })
}

fn run(out: &Path) -> io::Result<i32> {
    // A new directory is mandatory. Never reuse stale results or overwrite another run.
    fs::create_dir(out)?;
    let out = fs::canonicalize(out)?;
    let cohort = cohort::generate();
    let plan = plan::cases(&cohort);
    let mut report = Report::new();
    report.write(&out, &cohort, &plan)?;
    binary::write_coordinates(&out.join("coordinates.bin"), &cohort)?;

    #[cfg(any(target_os = "macos", probe_bridge_check))]
    let code = {
        let root = fs::canonicalize(std::env::current_dir()?)?;
        run_native(&root, &out, &cohort, &plan, &mut report)?
    };
    #[cfg(not(any(target_os = "macos", probe_bridge_check)))]
    let code = {
        report.status = "native_unavailable";
        report.support = "unavailable";
        report.shader_status = "not_attempted_requires_macos_xcode";
        report.native = Json::object([
            ("availability", Json::from("unavailable")),
            ("reason", Json::from("host_os_not_macos")),
        ]);
        report.required_missing = plan.iter().filter(|c| c.required()).count();
        report.optional_skipped = plan.iter().filter(|c| !c.required()).count();
        report.write(&out, &cohort, &plan)?;
        2
    };
    println!(
        "{}",
        Json::object([
            ("status", Json::from(report.status)),
            ("native_gpu_support", Json::from(report.support)),
            ("recorded_cases", Json::from(report.recorded_cases)),
            ("manifest", Json::from("manifest.json")),
            ("exit_code", Json::from(code)),
        ])
        .encode()
    );
    Ok(code)
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 || args[1] != "--out" || (args[0] != "run" && args[0] != "wrong-oracle") {
        eprintln!("{}", Json::object([
            ("status", Json::from("usage_error")),
            ("usage", Json::from("volume-sample-probe run --out NEW_DIRECTORY | wrong-oracle --out EXISTING_DIRECTORY")),
        ]).encode());
        std::process::exit(64);
    }
    let out = PathBuf::from(&args[2]);
    let code = if args[0] == "wrong-oracle" {
        match binary::read_white_anchor(&out.join("anchors.bin")) {
            Ok(white) => {
                let (code, report) = checks::wrong_oracle_report(white);
                println!("{}", report.encode());
                code
            }
            Err(error) => {
                println!(
                    "{}",
                    Json::object([
                        ("status", Json::from("control_input_invalid")),
                        ("error_kind", Json::from(format!("{:?}", error.kind()))),
                        ("exit_code", Json::from(2_i32)),
                    ])
                    .encode()
                );
                2
            }
        }
    } else {
        match run(&out) {
            Ok(code) => code,
            Err(error) => {
                eprintln!(
                    "{}",
                    Json::object([
                        ("status", Json::from("io_or_output_directory_failed")),
                        ("error_kind", Json::from(format!("{:?}", error.kind()))),
                        ("exit_code", Json::from(1_i32)),
                    ])
                    .encode()
                );
                1
            }
        }
    };
    std::process::exit(code);
}
