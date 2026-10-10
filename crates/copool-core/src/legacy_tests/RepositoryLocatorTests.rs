use crate::deployment::*;
#[test]
fn testProxydManifestURLPrefersCurrentSourceLayout() {
    let root = tempfile::tempdir().unwrap();
    let current = root
        .path()
        .join("Sources/Copool/Resources/proxyd-src/proxyd/Cargo.toml");
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
        "Sources/Copool/Resources/proxyd-src/proxyd/Cargo.toml",
        "Sources/Copool/App/AppContainer.swift",
    ] {
        let p = root.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "").unwrap();
    }
    assert_eq!(
        find_root(&root.path().join("Sources/Copool/App/AppContainer.swift")),
        Some(root.path().into())
    );
}
