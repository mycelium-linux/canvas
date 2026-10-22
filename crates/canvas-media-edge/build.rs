#[cfg(target_os = "macos")]
fn main() {
    use std::process::Command;

    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    let output = Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .expect("xcrun is required to locate the Swift runtime");
    assert!(output.status.success(), "xcrun could not locate swiftc");
    let swiftc = String::from_utf8(output.stdout).expect("xcrun returned a non-UTF-8 path");
    let runtime = std::path::Path::new(swiftc.trim())
        .parent()
        .and_then(std::path::Path::parent)
        .expect("swiftc path has no toolchain lib directory")
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime.display());
}

#[cfg(not(target_os = "macos"))]
fn main() {}
