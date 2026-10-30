use std::{fs, io, path::Path, process::Command};

#[path = "../src/json.rs"]
mod json;
use json::Json;

// Reuse command reporting, byte limits and path auditing without calling the
// numeric compiler or changing its public source/API. No numeric harness is linked.
#[allow(dead_code)]
mod artifact_tools {
    include!("../src/artifacts.rs");

    pub fn checked(command: &mut Command, root: &Path, out: &Path) -> io::Result<Json> {
        let result = invoke(command, &Privacy::new(root, out));
        if result.ok { Ok(result.report) } else { Err(io::Error::other(result.report.encode())) }
    }

    pub fn publish(source: &Path, destination: &Path, root: &Path, out: &Path) -> io::Result<Json> {
        publish_artifact(source, destination, &Privacy::new(root, out)).map_err(io::Error::other)
    }
}

const SOURCE: &str = "shaders/pure-return/texture3d.metal";
const STANDARDS: [&str; 2] = ["metal3.1", "metal4.1"];
const FORMS: [&str; 5] = ["implicit", "bias", "level0", "level0_edge", "level0_zero"];

fn xcrun(directory: &Path, args: &[String]) -> Command {
    let mut command = Command::new("xcrun");
    command.current_dir(directory).args(["--sdk", "macosx"]).args(args)
        .env_remove("MACOSX_DEPLOYMENT_TARGET").env_remove("SDKROOT");
    command
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).into()).collect()
}

fn text_field(report: &Json, name: &str) -> String {
    if let Json::Object(fields) = report {
        for (key, value) in fields {
            if key == name {
                if let Json::String(text) = value { return text.trim().into(); }
            }
        }
    }
    String::new()
}

fn require_sdk(sdk: &str) -> io::Result<()> {
    if sdk.split('.').next() != Some("27") {
        return Err(io::Error::other("requires macOS SDK 27; no fallback"));
    }
    Ok(())
}

fn compile_args(standard: &str, build: &str, air: &str) -> Vec<String> {
    vec![
        "metal".into(), format!("-std={standard}"), "-fno-fast-math".into(), "-g0".into(),
        format!("-ffile-prefix-map={build}=/probe"),
        format!("-fdebug-prefix-map={build}=/probe"),
        "-fdebug-compilation-dir=/probe".into(), "-c".into(), "texture3d.metal".into(),
        "-o".into(), air.into(),
    ]
}

fn compile(root: &Path, out: &Path) -> io::Result<()> {
    if fs::symlink_metadata(out).is_ok() {
        return Err(io::Error::other("output directory must not exist"));
    }
    if !cfg!(target_os = "macos") {
        return Err(io::Error::other("requires macOS and installed Xcode; no fallback or download"));
    }
    let sdk = artifact_tools::checked(&mut xcrun(root, &strings(&["--show-sdk-version"])), root, out)?;
    require_sdk(&text_field(&sdk, "stdout"))?;
    let metal = artifact_tools::checked(&mut xcrun(root, &strings(&["metal", "--version"])), root, out)?;
    let metallib = artifact_tools::checked(&mut xcrun(root, &strings(&["metallib", "--version"])), root, out)?;
    let source = fs::read(root.join(SOURCE))?;
    fs::create_dir(out)?;
    let build = out.join(".build");
    fs::create_dir(&build)?;
    fs::write(build.join("texture3d.metal"), &source)?;
    let mut files = Vec::new();
    for standard in STANDARDS {
        let stem = format!("texture3d-{}", standard.replace('.', ""));
        let air = format!("{stem}.air");
        let library = format!("{stem}.metallib");
        let args = compile_args(standard, &build.to_string_lossy(), &air);
        artifact_tools::checked(&mut xcrun(&build, &args), root, out)?;
        artifact_tools::checked(&mut xcrun(&build, &strings(&["metallib", &air, "-o", &library])), root, out)?;
        files.push((standard, air, library));
    }
    // Neither profile is published until both compile/link and all byte audits pass.
    // Only generic input is used; path remapping is not confidentiality.
    let pending = out.join(".publish");
    fs::create_dir(&pending)?;
    let mut profiles = Vec::new();
    for (standard, air, library) in &files {
        let artifacts = [air, library].into_iter().map(|name| {
            artifact_tools::publish(&build.join(name), &pending, root, out)
        }).collect::<io::Result<Vec<_>>>()?;
        profiles.push(Json::object([
            ("standard", Json::from(*standard)),
            ("compile", Json::Array(compile_args(standard, "<build>", air).into_iter().map(Json::from).collect())),
            ("link", Json::Array(strings(&["metallib", air, "-o", library]).into_iter().map(Json::from).collect())),
            ("artifacts", Json::Array(artifacts)),
        ]));
    }
    let entries = ["float", "half"].into_iter().flat_map(|scalar| {
        FORMS.into_iter().map(move |form| Json::from(format!("v3_{form}_{scalar}")))
    }).collect();
    let manifest = Json::object([
        ("schema", Json::from(1u32)),
        ("kind", Json::from("compile-only")),
        ("source", Json::from(SOURCE)),
        ("source_bytes", Json::from(source.len())),
        ("entry_points", Json::Array(entries)),
        ("toolchain", Json::object([("sdk", sdk), ("metal", metal), ("metallib", metallib)])),
        ("tool_prefix", Json::Array(strings(&["xcrun", "--sdk", "macosx"]).into_iter().map(Json::from).collect())),
        ("deployment", Json::from("compiler default; MACOSX_DEPLOYMENT_TARGET and SDKROOT unset")),
        ("air_signature_inspection", Json::from("pending_external_inspection")),
        ("profiles", Json::Array(profiles)),
    ]);
    fs::rename(&pending, out.join("shaders"))?;
    fs::write(out.join("manifest.json"), manifest.encode() + "\n")?;
    for (_, air, library) in &files {
        fs::remove_file(build.join(air))?;
        fs::remove_file(build.join(library))?;
    }
    fs::remove_file(build.join("texture3d.metal"))?;
    fs::remove_dir(build)?;
    Ok(())
}

fn main() {
    let result = (|| {
        let root = std::env::current_dir()?;
        let mut args = std::env::args_os().skip(1);
        let out = root.join(args.next().unwrap_or_else(|| "out-pure-return".into()));
        if args.next().is_some() { return Err(io::Error::other("usage: compile-pure-return [new-output-directory]")); }
        compile(&root, &out)
    })();
    if let Err(error) = result {
        eprintln!("pure-return compilation failed: {error}");
        std::process::exit(1);
    }
    println!("compiled both standards; no GPU execution");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_standards_no_minimum_os_or_target() {
        assert_eq!(STANDARDS, ["metal3.1", "metal4.1"]);
        for standard in STANDARDS {
            let args = compile_args(standard, "/probe/build", "test.air");
            assert!(args.contains(&format!("-std={standard}")));
            assert!(args.contains(&"-fno-fast-math".into()));
            assert!(!args.iter().any(|arg| arg.contains("version-min") || arg.starts_with("-target")));
            assert_eq!(args.iter().filter(|arg| arg.ends_with(".metal")).count(), 1);
        }
    }

    #[test]
    fn deployment_environment_is_removed_per_tool_call() {
        let command = xcrun(Path::new("."), &strings(&["metal", "--version"]));
        for name in ["MACOSX_DEPLOYMENT_TARGET", "SDKROOT"] {
            assert!(command.get_envs().any(|(key, value)| key == name && value.is_none()));
        }
    }

    #[test]
    fn sdk_gate_fails_closed() {
        assert!(require_sdk("27.0").is_ok());
        for sdk in ["26.0", "127.0", "", "28.0"] { assert!(require_sdk(sdk).is_err()); }
    }

    #[test]
    fn source_is_ten_standalone_pure_return_fragments() {
        let source = include_str!("../shaders/pure-return/texture3d.metal");
        assert_eq!(source.matches("fragment ").count(), 10);
        assert_eq!(source.matches("return t.sample(").count(), 10);
        assert_eq!(source.matches("float3(in.uv, 0.5f)").count(), 10);
        assert_eq!(source.matches("bias(in.uv.x)").count(), 2);
        assert_eq!(source.matches("level(0.0f)").count(), 6);
        for scalar in ["float", "half"] {
            for form in FORMS { assert!(source.contains(&format!("v3_{form}_{scalar}("))); }
        }
        for denied in ["device ", "[[buffer(", "probe_store", "#include \"", ".write("] {
            assert!(!source.contains(denied));
        }
        assert!(source.contains("address::clamp_to_edge, filter::nearest"));
        assert!(source.contains("address::clamp_to_zero, filter::nearest"));
    }

    #[test]
    fn missing_sdk_stdout_does_not_qualify() {
        assert!(require_sdk(&text_field(&Json::Null, "stdout")).is_err());
        assert_eq!(text_field(&Json::object([("stdout", Json::from("27.0\n"))]), "stdout"), "27.0");
    }
}
