use std::process::Command;
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=native/widget_bridge.swift");
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        let target = format!("{arch}-apple-macosx14.0");
        let library = out.join("libCopoolWidgetBridge.a");
        let result = Command::new("xcrun")
            .args([
                "swiftc",
                "-parse-as-library",
                "-emit-library",
                "-static",
                "-module-name",
                "CopoolWidgetBridge",
                "-target",
                &target,
                "-module-cache-path",
            ])
            .arg(out.join("swift-module-cache"))
            .arg("native/widget_bridge.swift")
            .arg("-o")
            .arg(&library)
            .status()
            .expect("Swift compiler is required for WidgetKit");
        assert!(result.success(), "WidgetKit bridge compilation failed");
        let sdk = Command::new("xcrun")
            .args(["--sdk", "macosx", "--show-sdk-path"])
            .output()
            .expect("macOS SDK not found");
        let sdk = String::from_utf8(sdk.stdout).unwrap();
        println!("cargo:rustc-link-search=native={}", out.display());
        println!(
            "cargo:rustc-link-search=native={}/usr/lib/swift",
            sdk.trim()
        );
        println!("cargo:rustc-link-lib=static=CopoolWidgetBridge");
        // Mach-O objects carry LC_LINKER_OPTION entries; Apple's linker reads
        // Swift's runtime/framework requirements directly from the archive.
        let compiler = Command::new("xcrun")
            .args(["--find", "swiftc"])
            .output()
            .expect("Swift compiler path unavailable");
        let compiler = std::path::PathBuf::from(String::from_utf8(compiler.stdout).unwrap().trim());
        let runtime = compiler
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("lib/swift/macosx");
        println!("cargo:rustc-link-search=native={}", runtime.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    tauri_build::build()
}
