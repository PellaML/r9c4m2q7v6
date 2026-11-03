use std::{fs, io, path::Path, process::Command};

fn run(command: &mut Command) -> io::Result<()> {
    if command.status()?.success() { Ok(()) }
    else { Err(io::Error::other("native evidence command failed")) }
}

fn main() -> io::Result<()> {
    let out = Path::new("out-native-service");
    fs::create_dir(out)?;
    // Observed fixed public system path. No account/user files are collected.
    let input = Path::new("/System/Library/Frameworks/ParavirtualizedGraphics.framework/Versions/A/XPCServices/com.apple.gpusw.ParavirtualizedGraphicsGPUTask.xpc/Contents/MacOS/com.apple.gpusw.ParavirtualizedGraphicsGPUTask");
    let length = fs::metadata(input)?.len();
    if !(1..=8 * 1024 * 1024).contains(&length) {
        return Err(io::Error::other("native service size outside bound"));
    }
    println!("native_service_bytes={length}");
    run(&mut Command::new("sw_vers"))?;
    run(Command::new("uname").arg("-m"))?;
    run(Command::new("shasum").args(["-a", "256"]).arg(input))?;
    // Only encrypted bytes leave the runner. The private key stays with recipient.
    run(Command::new("openssl").args(["cms", "-encrypt", "-binary", "-aes-256-cbc", "-in"])
        .arg(input).arg("-out").arg(out.join("service.cms"))
        .args(["-outform", "DER", "scripts/native-service/recipient.cert.pem"]))?;
    run(Command::new("shasum").args(["-a", "256"]).arg(out.join("service.cms")))?;
    Ok(())
}
