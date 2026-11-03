use std::{fs, io, path::Path, process::Command};

fn run(command: &mut Command) -> io::Result<()> {
    if command.status()?.success() { Ok(()) }
    else { Err(io::Error::other("native evidence command failed")) }
}

fn main() -> io::Result<()> {
    let out = Path::new("out-native-service");
    fs::create_dir(out)?;
    // Fixed public system-service paths only: no project, account or user files.
    let roots = ["/System/Library/Frameworks", "/System/Library/PrivateFrameworks"];
    let mut selected = None;
    for root in roots {
        let path = Path::new(root).join("ParavirtualizedGraphics.framework/Versions/A/XPCServices/GPUTask.xpc/Contents/MacOS/GPUTask");
        if path.is_file() { selected = Some(path); break; }
    }
    let input = selected.ok_or_else(|| io::Error::other("fixed native service unavailable"))?;
    let length = fs::metadata(&input)?.len();
    if !(1..=8 * 1024 * 1024).contains(&length) {
        return Err(io::Error::other("native service size outside bound"));
    }
    println!("native_service_bytes={length}");
    run(&mut Command::new("sw_vers"))?;
    run(Command::new("uname").arg("-m"))?;
    run(Command::new("shasum").args(["-a", "256"]).arg(&input))?;
    // The public artifact contains encrypted source bytes only. The private key
    // is never installed on this runner or committed to this repository.
    run(Command::new("openssl").args(["cms", "-encrypt", "-binary", "-aes-256-cbc", "-in"])
        .arg(&input).arg("-out").arg(out.join("service.cms"))
        .args(["-outform", "DER", "scripts/native-service/recipient.cert.pem"]))?;
    run(Command::new("shasum").args(["-a", "256"]).arg(out.join("service.cms")))?;
    Ok(())
}
