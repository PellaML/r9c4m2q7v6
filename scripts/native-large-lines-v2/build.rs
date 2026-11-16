use std::{env, fs, path::PathBuf, process::Command};

fn output(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .output()
        .expect("tool missing");
    assert!(out.status.success(), "tool failed (diagnostics kept local)");
    String::from_utf8(out.stdout)
        .expect("tool output is not UTF-8")
        .trim()
        .to_owned()
}

fn run(program: &str, args: &[&str]) {
    assert!(
        Command::new(program)
            .args(args)
            .status()
            .expect("tool missing")
            .success(),
        "native build failed"
    );
}

fn main() {
    for file in [
        "native/metal_probe.h",
        "native/metal_probe.m",
        "native/probe.metal",
        "build.rs",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    println!(
        "cargo:rustc-env=PROBE_RUSTC={}",
        output(&rustc, &["--version"])
    );
    println!(
        "cargo:rustc-env=PROBE_TARGET={}",
        env::var("TARGET").unwrap()
    );
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        println!("cargo:rustc-env=PROBE_SDK_VERSION=unavailable");
        println!("cargo:rustc-env=PROBE_SDK_BUILD=unavailable");
        return;
    }
    assert_eq!(
        env::var("HOST").unwrap(),
        env::var("TARGET").unwrap(),
        "native builds only; no cross compilation"
    );
    for (key, flag) in [
        ("PROBE_SDK_VERSION", "--show-sdk-version"),
        ("PROBE_SDK_BUILD", "--show-sdk-build-version"),
    ] {
        println!(
            "cargo:rustc-env={key}={}",
            output("/usr/bin/xcrun", &["--sdk", "macosx", flag])
        );
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = fs::read("native/probe.metal").expect("missing authored MSL");
    let mut header = String::from("static const char generic_msl[] = {\n");
    for byte in source {
        header.push_str(&format!("{byte},"));
    }
    header.push_str("0};\n");
    fs::write(out.join("metal_source.h"), header).unwrap();
    let object = out.join("metal_probe.o");
    let archive = out.join("libgeneric_metal_probe.a");
    let sdk = output("/usr/bin/xcrun", &["--sdk", "macosx", "--show-sdk-path"]);
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => panic!("unsupported native architecture"),
    };
    run(
        "/usr/bin/xcrun",
        &[
            "--sdk",
            "macosx",
            "clang",
            "-isysroot",
            &sdk,
            "-arch",
            arch,
            "-mmacosx-version-min=13.0",
            "-std=c11",
            "-fobjc-arc",
            "-fblocks",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wno-deprecated-declarations",
            "-I",
            out.to_str().unwrap(),
            "-c",
            "native/metal_probe.m",
            "-o",
            object.to_str().unwrap(),
        ],
    );
    run(
        "/usr/bin/xcrun",
        &[
            "ar",
            "rcs",
            archive.to_str().unwrap(),
            object.to_str().unwrap(),
        ],
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=generic_metal_probe");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=Metal");
}
