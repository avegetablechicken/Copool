# Copool Repository Rules

Copool is a Rust/Tauri desktop app: `crates/copool-core` (accounts, usage, proxy, remote servers), `desktop` (Tauri shell, `desktop/ui` web UI, `desktop/widget` native macOS WidgetKit extension) and `resources` (shared proxy engine and daemon, prebuilt daemons, localization).

## Local App Bundle Build

- For requests to build the application locally, run from the repository root:

```bash
python3 scripts/build_rust_desktop.py > /tmp/copool-app-bundle-build.log 2>&1
```

- On macOS this checks the UI, builds a Release `Copool.app` with the Tauri CLI (its pre-bundle hook builds the WidgetKit extension from `desktop/widget/CopoolWidgets.xcodeproj`), signs the widget and then the app ad hoc with the entitlements in `desktop/native`, and verifies the signature, architecture and version. Elsewhere it builds the executable only.
- Xcode and the Tauri CLI may require execution outside the sandbox to access system build services and caches; use the execution tool's approval mechanism when needed.
- Wait for a successful exit before delivering; on failure, inspect `/tmp/copool-app-bundle-build.log`. Deliver `target/release/bundle/macos/Copool.app` as a clickable absolute path, and report the verified architectures and version instead of assuming them.
- This is a locally ad-hoc-signed build, not a notarized distribution release. An ordinary build does not include installing, launching, replacing an installed app, committing, uploading, or publishing. Perform those only when requested.

## Development

- Test with `python3 scripts/test_rust.py` (formatting, `cargo test -p copool-core --locked`, UI syntax and `desktop/tests`, the standalone daemon), or the individual commands; build with `cargo build -p copool-desktop --locked`.
- The core and packaged daemon share Rust sources in `resources/proxyd-src/src`. After modifying these sources, also check the standalone daemon with `cargo check --manifest-path resources/proxyd-src/proxyd/Cargo.toml --locked`.
- Localized strings live in `resources/locales/*.lproj/Localizable.strings`; regenerate `desktop/ui/locales.js` with `python3 scripts/sync_rust_locales.py` after editing them.
- Use temporary `COPOOL_DATA_DIR` and `COPOOL_CODEX_AUTH_FILE` paths for interactive smoke tests. Never import or switch the user's real credentials as part of testing.
- Keep limitations and platform validation status current in `docs/rust-cross-platform.md`.

- The GUI must preserve the original SwiftUI app's layout, controls and interactions. Its sources were removed from the tree; consult them in history at commit `cae2fce` (for example `git show cae2fce:Sources/Copool/Features/Accounts/AccountsPageModel.swift`). References such as Coport inform native material and rendering implementation; they do not authorize redesigning Copool.
