use crate::deployment::*;
#[test]
fn testProxydManifestURLPrefersCurrentSourceLayout() {
    let root = tempfile::tempdir().unwrap();
    let current = root.path().join("resources/proxyd-src/proxyd/Cargo.toml");
    let legacy = root.path().join("src-tauri/proxyd/Cargo.toml");
    for p in [&current, &legacy] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    assert_eq!(manifest(root.path()), Some(current));
}
#[test]
fn testFindRepoRootDiscoversCurrentSourceLayoutFromNestedFile() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "resources/proxyd-src/proxyd/Cargo.toml",
        "desktop/src/main.rs",
    ] {
        let p = root.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    assert_eq!(
        find_root(&root.path().join("desktop/src/main.rs")),
        Some(root.path().into())
    );
}
