use std::{env, fs, path::PathBuf, process::Command};

#[path = "src/json.rs"]
mod json;
use json::Json;

fn tool(program: &str, args: &[&str]) -> Option<String> {
    let result = Command::new(program).args(args).output().ok()?;
    if !result.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(probe_bridge_check)");
    println!("cargo:rerun-if-changed=bridge/metal_bridge.m");
    println!("cargo:rerun-if-changed=bridge/metal_bridge.h");
    println!("cargo:rerun-if-changed=src/json.rs");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let mac = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let rust_version = tool(&rustc, &["--version", "--verbose"]).unwrap_or_default();
    let clang = if mac {
        tool("xcrun", &["--sdk", "macosx", "clang", "--version"])
            .and_then(|s| s.lines().next().map(str::to_string))
    } else {
        None
    };
    let info = Json::object([
        ("rustc", Json::from(rust_version)),
        ("target", Json::from(env::var("TARGET").unwrap_or_default())),
        ("native_bridge_compiled", Json::from(mac)),
        ("clang", clang.map(Json::from).unwrap_or(Json::Null)),
        ("deployment_target", Json::from("13.0")),
        ("bridge_optimization", Json::from("-O2 -g0")),
    ]);
    fs::write(
        output.join("build_info.rs"),
        format!("pub const BUILD_INFO: &str = {:?};\n", info.encode()),
    )
    .expect("write build metadata");
    if !mac {
        return;
    }

    let status = Command::new("xcrun")
        .current_dir(&root)
        .args([
            "--sdk",
            "macosx",
            "clang",
            "-c",
            "bridge/metal_bridge.m",
            "-fobjc-arc",
            "-std=c11",
            "-O2",
            "-g0",
            "-Wall",
            "-Wextra",
            "-Werror=implicit-function-declaration",
            "-mmacosx-version-min=13.0",
        ])
        .arg(format!("-ffile-prefix-map={}=/probe", root.display()))
        .arg(format!("-fdebug-prefix-map={}=/probe", root.display()))
        .arg("-o")
        .arg(output.join("metal_bridge.o"))
        .status()
        .expect("Xcode clang is required; no toolchain download is attempted");
    assert!(status.success(), "native bridge compilation failed");
    let status = Command::new("xcrun")
        .args(["--sdk", "macosx", "ar", "crs"])
        .arg(output.join("libprobe_metal.a"))
        .arg(output.join("metal_bridge.o"))
        .status()
        .expect("Xcode ar is required");
    assert!(status.success(), "native bridge archive failed");
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=probe_metal");
    println!("cargo:rustc-link-lib=framework=Metal");
    println!("cargo:rustc-link-lib=framework=Foundation");
}
