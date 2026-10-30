use crate::json::Json;
use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

pub const MODULES: [&str; 10] = [
    "read3d_level0_float",
    "read3d_level0_half",
    "read3d_implicit_float",
    "read3d_bias_float",
    "read3d_implicit_half",
    "read3d_bias_half",
    "read3d_level0_float_constexpr",
    "read3d_level0_half_constexpr",
    "read3d_level0_float_pixel_constexpr",
    "read3d_level0_half_pixel_constexpr",
];
pub const FLAGS: [&str; 5] = [
    "-std=metal3.0",
    "-O2",
    "-g0",
    "-fno-fast-math",
    "-mmacosx-version-min=13.0",
];

pub struct Artifact {
    pub name: &'static str,
    pub required: bool,
    pub library: Option<PathBuf>,
    pub report: Json,
}

struct Privacy {
    paths: Vec<String>,
}
impl Privacy {
    fn new(root: &Path, out: &Path) -> Self {
        let mut paths = vec![
            root.to_string_lossy().into_owned(),
            out.to_string_lossy().into_owned(),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            paths.push(home.to_string_lossy().into_owned());
        }
        paths.retain(|p| p.len() > 1);
        paths.sort_by_key(|p| std::cmp::Reverse(p.len()));
        paths.dedup();
        Self { paths }
    }
    fn text(&self, text: &[u8]) -> String {
        let mut s = String::from_utf8_lossy(text).into_owned();
        for path in &self.paths {
            s = s.replace(path, "/probe");
        }
        s.lines()
            .map(|line| {
                if line.starts_with("InstalledDir:") {
                    "InstalledDir: <system-toolchain>"
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(4096)
            .collect()
    }
    fn safe_bytes(&self, bytes: &[u8]) -> bool {
        self.paths.iter().all(|path| {
            let utf16: Vec<u8> = path.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let utf16_be: Vec<u8> = path.encode_utf16().flat_map(u16::to_be_bytes).collect();
            !contains(bytes, path.as_bytes())
                && !contains(bytes, &utf16)
                && !contains(bytes, &utf16_be)
        })
    }
}
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|slice| slice == needle)
}

struct Tool {
    ok: bool,
    report: Json,
}
fn invoke(command: &mut Command, privacy: &Privacy) -> Tool {
    match command.output() {
        Ok(result) => Tool {
            ok: result.status.success(),
            report: Json::object([
                ("success", Json::from(result.status.success())),
                (
                    "exit_code",
                    result.status.code().map(Json::from).unwrap_or(Json::Null),
                ),
                ("stdout", Json::from(privacy.text(&result.stdout))),
                ("stderr", Json::from(privacy.text(&result.stderr))),
            ]),
        },
        Err(error) => Tool {
            ok: false,
            report: Json::object([
                ("success", Json::from(false)),
                (
                    "spawn_error_kind",
                    Json::from(format!("{:?}", error.kind())),
                ),
            ]),
        },
    }
}

pub fn compile(root: &Path, out: &Path) -> io::Result<(Vec<Artifact>, Json)> {
    let privacy = Privacy::new(root, out);
    let source = root.join("shaders");
    let staging = out.join(".shader-build");
    let destination = out.join("shaders");
    fs::create_dir(&staging)?;
    fs::create_dir(&destination)?;
    let version_specs: [(&str, &str, &[&str]); 4] = [
        ("xcode", "xcodebuild", &["-version"]),
        ("sdk", "xcrun", &["--sdk", "macosx", "--show-sdk-version"]),
        ("metal", "xcrun", &["--sdk", "macosx", "metal", "--version"]),
        (
            "metallib",
            "xcrun",
            &["--sdk", "macosx", "metallib", "--version"],
        ),
    ];
    let versions = Json::Object(
        version_specs
            .iter()
            .map(|(key, program, args)| {
                let result = invoke(Command::new(program).args(*args), &privacy);
                (key.to_string(), result.report)
            })
            .collect(),
    );

    let mut artifacts = Vec::new();
    for (index, name) in MODULES.iter().enumerate() {
        let air = staging.join(format!("{name}.air"));
        let library = staging.join(format!("{name}.metallib"));
        let mut command = Command::new("xcrun");
        command
            .current_dir(&source)
            .args(["--sdk", "macosx", "metal"])
            .args(FLAGS)
            .arg(format!("-ffile-prefix-map={}=/probe", root.display()))
            .arg(format!("-fdebug-prefix-map={}=/probe", root.display()))
            .arg("-fdebug-compilation-dir=/probe/shaders")
            .arg("-c")
            .arg(format!("{name}.metal"))
            .arg("-o")
            .arg(&air);
        let compiled = invoke(&mut command, &privacy);
        let mut status = "compile_failed";
        let mut air_report = Json::Null;
        let mut library_report = Json::Null;
        let mut link_report = Json::Null;
        let mut usable = None;
        if compiled.ok {
            match publish_artifact(&air, &destination, &privacy) {
                Ok(report) => {
                    air_report = report;
                    let linked = invoke(
                        Command::new("xcrun")
                            .current_dir(&source)
                            .args(["--sdk", "macosx", "metallib"])
                            .arg(&air)
                            .arg("-o")
                            .arg(&library),
                        &privacy,
                    );
                    link_report = linked.report;
                    status = "metallib_link_failed";
                    if linked.ok {
                        match publish_artifact(&library, &destination, &privacy) {
                            Ok(report) => {
                                library_report = report;
                                usable = Some(destination.join(format!("{name}.metallib")));
                                status = "ready";
                            }
                            Err(reason) => status = reason,
                        }
                    }
                }
                Err(reason) => status = reason,
            }
        }
        // Only audited copies in shaders/ are publishable. Quarantine is never uploaded.
        let _ = fs::remove_file(&air);
        let _ = fs::remove_file(&library);
        artifacts.push(Artifact {
            name,
            required: index < 6,
            library: usable,
            report: Json::object([
                ("module", Json::from(*name)),
                ("entry_point", Json::from(*name)),
                ("stage", Json::from("fragment")),
                ("required", Json::from(index < 6)),
                ("status", Json::from(status)),
                (
                    "compile_flags",
                    Json::Array(FLAGS.iter().map(|v| Json::from(*v)).collect()),
                ),
                ("source_paths_remapped_to_public_probe_prefix", Json::from(true)),
                ("compile", compiled.report),
                ("link", link_report),
                ("air", air_report),
                ("metallib", library_report),
                (
                    "air_signature_inspection",
                    Json::from("pending_external_inspection"),
                ),
            ]),
        });
    }
    let _ = fs::remove_dir(&staging);
    Ok((artifacts, versions))
}

fn publish_artifact(
    source: &Path,
    destination: &Path,
    privacy: &Privacy,
) -> Result<Json, &'static str> {
    let metadata = fs::symlink_metadata(source).map_err(|_| "compiler_output_missing")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 4 * 1024 * 1024 {
        return Err("artifact_size_or_type_rejected");
    }
    let bytes = fs::read(source).map_err(|_| "artifact_read_failed")?;
    if !privacy.safe_bytes(&bytes) {
        return Err("artifact_absolute_path_audit_failed");
    }
    let name: OsString = source
        .file_name()
        .ok_or("artifact_name_missing")?
        .to_os_string();
    fs::write(destination.join(&name), &bytes).map_err(|_| "artifact_write_failed")?;
    Ok(Json::object([
        (
            "file",
            Json::from(format!("shaders/{}", name.to_string_lossy())),
        ),
        ("bytes", Json::from(bytes.len())),
        (
            "source_and_output_and_home_path_scan_passed",
            Json::from(true),
        ),
        ("debug_or_record_sources_requested", Json::from(false)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_audit_and_diagnostic_redaction_are_not_noops() {
        let privacy = Privacy {
            paths: vec!["/home/example/probe".into()],
        };
        assert!(!privacy.safe_bytes(b"prefix /home/example/probe/source suffix"));
        let utf16: Vec<u8> = "/home/example/probe"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(!privacy.safe_bytes(&utf16));
        assert!(privacy.safe_bytes(b"read3d_level0_float.metal"));
        assert_eq!(privacy.text(b"/home/example/probe/source"), "/probe/source");
    }
    #[test]
    fn independent_runtime_artifacts_include_every_requested_signature_family() {
        for operation in ["implicit", "bias", "level0"] {
            for scalar in ["float", "half"] {
                assert!(MODULES.contains(&format!("read3d_{operation}_{scalar}").as_str()));
            }
        }
        assert!(!FLAGS
            .iter()
            .any(|flag| flag.contains("record-sources") || *flag == "-g"));
    }
}
