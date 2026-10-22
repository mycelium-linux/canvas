//! Owner-side native Canvas payload. Debian derives runtime library dependencies.
//! No maintainer scripts: package delivery never selects/enables a compositor.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn output(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let mut c = Command::new(program);
    c.args(args).env("LC_ALL", "C");
    if let Some(cwd) = cwd {
        c.current_dir(cwd);
    }
    let out = c.output()?;
    if !out.status.success() {
        return Err(format!(
            "{program}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(out.stdout)?)
}
fn architecture(bytes: &[u8]) -> Result<&'static str> {
    if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
        return Err("expected 64-bit little-endian ELF".into());
    }
    match u16::from_le_bytes([bytes[18], bytes[19]]) {
        62 => Ok("amd64"),
        183 => Ok("arm64"),
        _ => Err("unsupported ELF architecture".into()),
    }
}
fn version() -> Result<String> {
    let manifest = include_str!("../../../Cargo.toml");
    let section = manifest
        .split("[workspace.package]")
        .nth(1)
        .ok_or("no workspace package")?
        .split("[workspace.dependencies]")
        .next()
        .unwrap();
    let value = section
        .lines()
        .find_map(|line| {
            line.strip_prefix("version = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .ok_or("no workspace version")?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b".+~-".contains(&b))
    {
        return Err("invalid package version".into());
    }
    Ok(value.into())
}
fn build(binary: &Path, directory: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let binary = binary.canonicalize()?;
    let arch = architecture(&fs::read(&binary)?)?;
    if output("dpkg", &["--print-architecture"], None)?.trim() != arch {
        return Err("cross-architecture shlib resolution requires a target Debian sysroot; native packaging only in this slice".into());
    }
    let version = version()?;
    fs::create_dir_all(directory)?;
    let directory = directory.canonicalize()?;
    let destination = directory.join(format!("canvas-linux_{version}_{arch}.deb"));
    if destination.exists() {
        return Err("refusing to overwrite package".into());
    }
    let stage = PathBuf::from(
        output(
            "mktemp",
            &["-d", directory.join(".canvas-deb.XXXXXX").to_str().unwrap()],
            None,
        )?
        .trim(),
    );
    // Keep failed staging for diagnosis. Remove only our freshly created directory
    // after an archive has been published successfully.
    fs::create_dir(stage.join("debian"))?;
    fs::write(stage.join("debian/control"),"Source: canvas-linux\nSection: x11\nPriority: optional\nMaintainer: FPL <software@fpl.dev>\n\nPackage: canvas-linux\nArchitecture: any\nDescription: Native Canvas Wayland compositor\n")?;
    let deps = output(
        "dpkg-shlibdeps",
        &["-O", &format!("-e{}", binary.display())],
        Some(&stage),
    )?;
    let dependencies = deps
        .lines()
        .find_map(|l| l.strip_prefix("shlibs:Depends="))
        .ok_or("missing runtime dependencies")?;
    if dependencies.is_empty() || dependencies.contains('\n') {
        return Err("empty runtime dependency resolution".into());
    }
    fs::create_dir(stage.join("DEBIAN"))?;
    fs::write(stage.join("DEBIAN/control"),format!("Package: canvas-linux\nVersion: {version}\nArchitecture: {arch}\nSection: x11\nPriority: optional\nMaintainer: FPL <software@fpl.dev>\nDepends: {dependencies}, adwaita-icon-theme\nDescription: Native Canvas Wayland compositor\n Owner-controlled compositor and native control CLI.\n"))?;
    fs::create_dir_all(stage.join("usr/bin"))?;
    fs::copy(&binary, stage.join("usr/bin/canvas-linux"))?;
    for dir in ["", "DEBIAN", "usr", "usr/bin"] {
        fs::set_permissions(stage.join(dir), fs::Permissions::from_mode(0o755))?;
    }
    fs::set_permissions(
        stage.join("DEBIAN/control"),
        fs::Permissions::from_mode(0o644),
    )?;
    fs::set_permissions(
        stage.join("usr/bin/canvas-linux"),
        fs::Permissions::from_mode(0o755),
    )?;
    // debian/control is build-only, never part of the archive.
    fs::remove_dir_all(stage.join("debian"))?;
    let archive = directory.join(format!(
        "{}.deb",
        stage.file_name().unwrap().to_str().unwrap()
    ));
    output(
        "dpkg-deb",
        &[
            "--root-owner-group",
            "--build",
            stage.to_str().unwrap(),
            archive.to_str().unwrap(),
        ],
        None,
    )?;
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o644))?;
    fs::hard_link(&archive, &destination)?;
    fs::remove_file(archive)?;
    fs::remove_dir_all(stage)?;
    Ok(destination)
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: canvas-deb CANVAS_LINUX_EXECUTABLE OUTPUT_DIRECTORY".into());
    }
    println!(
        "{}",
        build(Path::new(&args[0]), Path::new(&args[1]))?.display()
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_elf_and_derives_both_architectures() {
        assert!(architecture(b"not ELF").is_err());
        let mut data = [0u8; 20];
        data[..4].copy_from_slice(b"\x7fELF");
        data[4] = 2;
        data[5] = 1;
        for (n, a) in [(62u16, "amd64"), (183, "arm64")] {
            data[18..20].copy_from_slice(&n.to_le_bytes());
            assert_eq!(architecture(&data).unwrap(), a);
        }
        data[4] = 1;
        assert!(architecture(&data).is_err());
    }
    #[test]
    fn package_version_comes_from_owner_workspace() {
        assert!(!version().unwrap().is_empty());
    }
}
