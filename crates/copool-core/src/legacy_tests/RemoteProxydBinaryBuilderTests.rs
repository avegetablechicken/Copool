use crate::deployment::*;
use std::{io::Write, path::Path};
const STUB: &str = "#!/bin/sh\nexit 0\n";
fn executable_stub(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, STUB).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
fn compressed_stub(resources: &Path, target: &str) {
    let path = resources
        .join("proxyd-prebuilt-archives")
        .join(target)
        .join("codex-tools-proxyd.zlib");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(STUB.as_bytes()).unwrap();
    std::fs::write(path, encoder.finish().unwrap()).unwrap();
}
#[test]
fn testBundledManifestPathRestoresFlattenedBundledSourceTree() {
    let root = tempfile::tempdir().unwrap();
    let resources = root.path().join("Resources");
    std::fs::create_dir_all(&resources).unwrap();
    for (name, body) in [
        (
            "Cargo.toml",
            "[package]\nname = \"codex-tools-proxyd\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        ),
        ("Cargo.lock", ""),
        (
            "main.rs",
            "#[path = \"../../src/auth.rs\"]\nmod auth;\nfn main() {}",
        ),
        ("auth.rs", "pub fn noop() {}"),
        ("models.rs", ""),
        ("proxy_daemon.rs", ""),
        ("proxy_service.rs", ""),
        ("state.rs", ""),
        ("store.rs", ""),
        ("usage.rs", ""),
        ("utils.rs", ""),
    ] {
        std::fs::write(resources.join(name), body).unwrap();
    }
    let path = bundled_manifest(&resources, &root.path().join("cache"))
        .unwrap()
        .unwrap();
    assert_eq!(path.file_name().unwrap(), "Cargo.toml");
    let crate_root = path.parent().unwrap();
    assert!(crate_root.join("src/main.rs").is_file());
    assert!(crate_root.parent().unwrap().join("src/auth.rs").is_file());
}
#[test]
fn testPrepareBinaryPathForBuildRemovesExistingCachedBinaryWhenForceRebuildEnabled() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("codex-tools-proxyd");
    executable_stub(&path);
    assert!(!prepare_binary(&path, true).unwrap());
    assert!(!path.exists());
}
#[test]
fn testPrepareBinaryPathForBuildKeepsExistingCachedBinaryWhenForceRebuildDisabled() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("codex-tools-proxyd");
    executable_stub(&path);
    assert!(prepare_binary(&path, false).unwrap());
    assert!(path.exists());
}
#[test]
fn testPrebuiltBinaryPrefersRepoRootArtifactForTarget() {
    let root = tempfile::tempdir().unwrap();
    let target = "x86_64-unknown-linux-musl";
    let path = root
        .path()
        .join("resources/proxyd-prebuilt")
        .join(target)
        .join("codex-tools-proxyd");
    executable_stub(&path);
    assert_eq!(
        prebuilt(Some(root.path()), None, root.path(), target).unwrap(),
        Some(path)
    );
}
#[test]
fn testPrebuiltBinaryUsesBundledCompressedArtifactForTarget() {
    let root = tempfile::tempdir().unwrap();
    let resources = root.path().join("Resources");
    let cache = root.path().join("ApplicationSupport");
    let target = "aarch64-unknown-linux-musl";
    compressed_stub(&resources, target);
    assert_eq!(
        prebuilt(None, Some(&resources), &cache, target).unwrap(),
        Some(
            cache
                .join("proxyd-prebuilt")
                .join(target)
                .join("codex-tools-proxyd")
        )
    );
}
#[test]
fn testPrebuiltBinaryExtractsBundledCompressedArtifactToCacheRoot() {
    let root = tempfile::tempdir().unwrap();
    let resources = root.path().join("Resources");
    let cache = root.path().join("ApplicationSupport");
    let target = "aarch64-unknown-linux-musl";
    compressed_stub(&resources, target);
    let resolved = prebuilt(None, Some(&resources), &cache, target)
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved,
        cache
            .join("proxyd-prebuilt")
            .join(target)
            .join("codex-tools-proxyd")
    );
    assert!(resolved.is_file());
    assert!(executable(&resolved));
    assert_eq!(std::fs::read_to_string(resolved).unwrap(), STUB);
}
