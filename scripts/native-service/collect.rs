use std::{fs, io, path::Path, process::Command};

fn run(command: &mut Command) -> io::Result<()> {
    if command.status()?.success() { Ok(()) }
    else { Err(io::Error::other("native evidence command failed")) }
}

fn locate(root: &Path, depth: usize, visited: &mut usize, matches: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    if depth > 20 || *visited > 500_000 { return Err(io::Error::other("system search bound exceeded")); }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied || error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        *visited += 1;
        let kind = entry.file_type()?;
        if kind.is_dir() { locate(&entry.path(), depth + 1, visited, matches)?; }
        else if kind.is_file() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.contains("gputask") { matches.push(path.clone()); }
            let text = path.to_string_lossy();
            if text.contains("ParavirtualizedGraphics.framework/") ||
               (text.contains("Virtualization.framework/") && text.contains("XPCServices/")) {
                println!("system_graphics_file={}", path.display());
            }
        }
        // Never follow filesystem symlinks or inspect account/user directories.
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let out = Path::new("out-native-service");
    fs::create_dir(out)?;
    // Locate one fixed executable name in public system roots, not user storage.
    let mut selected = Vec::new();
    let mut visited = 0;
    for root in ["/System/Library", "/usr/libexec"] {
        locate(Path::new(root), 0, &mut visited, &mut selected)?;
    }
    println!("system_entries_visited={visited}; candidates={}", selected.len());
    for path in &selected { println!("system_service_path={}", path.display()); }
    if selected.len() != 1 { return Err(io::Error::other("native service missing or ambiguous")); }
    let input = selected.remove(0);
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
