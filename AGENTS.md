# Copool Repository Rules

Copool is a Rust/Tauri desktop app: `crates/copool-core` (accounts, usage, proxy, remote servers), `desktop` (Tauri shell, `desktop/ui` web UI, `desktop/widget` native macOS WidgetKit extension) and `resources` (shared proxy engine and daemon, prebuilt daemons, localization).

## Local App Bundle Build

- For requests to build the application locally, run from the repository root:

```bash
python3 scripts/build_rust_desktop.py > /tmp/copool-app-bundle-build.log 2>&1
```

- On macOS this checks the UI, builds a Release `Copool.app` with the Tauri CLI (its pre-bundle hook builds the WidgetKit extension from `desktop/widget/CopoolWidgets.xcodeproj`), signs the widget and then the app with the entitlements in `desktop/native`, and verifies the signature, architecture and version. Signing uses the Apple Development certificate in the login keychain and the app group `<Team ID>.io.github.copool.rust`, else ad hoc with `group.io.github.copool.rust` (`scripts/macos_signing.py`; `COPOOL_SIGN_IDENTITY` overrides, `-` for ad hoc); the build's final line reports which. Elsewhere it builds the executable only.
- Xcode and the Tauri CLI may require execution outside the sandbox to access system build services and caches; use the execution tool's approval mechanism when needed.
- Wait for a successful exit before delivering; on failure, inspect `/tmp/copool-app-bundle-build.log`. Deliver `target/release/bundle/macos/Copool.app` as a clickable absolute path, and report the verified architectures and version instead of assuming them.
- This is a locally signed build, not a notarized distribution release. An ordinary build does not include installing, launching, replacing an installed app, committing, uploading, or publishing. Perform those only when requested.

## Development

- Test with `python3 scripts/test_rust.py` (formatting, `cargo test -p copool-core --locked`, UI syntax and `desktop/tests`, the standalone daemon), or the individual commands; build with `cargo build -p copool-desktop --locked`.
- The core and packaged daemon share Rust sources in `resources/proxyd-src/src`. After modifying these sources, also check the standalone daemon with `cargo check --manifest-path resources/proxyd-src/proxyd/Cargo.toml --locked`.
- Localized strings live in `resources/locales/*.lproj/Localizable.strings`; regenerate `desktop/ui/locales.js` with `python3 scripts/sync_rust_locales.py` after editing them.
- The app icon's source is the Icon Composer document `desktop/icons/Copool.icon`; regenerate the bundled PNG, ICNS and ICO and the README's `Copool.png` with `python3 scripts/build_app_icon.py` (macOS with Xcode) after editing it or the background stops in that script.
- Use temporary `COPOOL_DATA_DIR` and `COPOOL_CODEX_AUTH_FILE` paths for interactive smoke tests. Never import or switch the user's real credentials as part of testing.
- Keep limitations and platform validation status current in `docs/rust-cross-platform.md`.

- The GUI must preserve the original SwiftUI app's layout, controls and interactions. Its sources were removed from the tree; consult them in history at commit `cae2fce` (for example `git show cae2fce:Sources/Copool/Features/Accounts/AccountsPageModel.swift`). References such as Coport inform native material and rendering implementation; they do not authorize redesigning Copool.
- Plan tags are colored by tier so that comparable plans of every pool match: pink for the entry subscription, orange for those of $100 to $300, which their labels tell apart by multiplier, purple for Codex's $500 Pro 50x. `crates/copool-core/src/plan_tags.rs` alone names and colors plan tags and chooses workspace tags: `Application::accounts` gives each account its `planTag` and `workspaceTag`, which the UI only displays, and the widget snapshots are built from the same function. Do not derive plans in `desktop/ui` or `widgets.rs`.

## Windows Line Endings (Recurring CI Failure)

- Windows checkouts may use CRLF (`\r\n`). Do not assume files read from disk contain only LF (`\n`), even when tests pass on macOS or Linux.
- Tests and scripts that extract source functions, match multiline text, or compare text snapshots must normalize line endings before parsing or asserting, for example `fs.readFileSync(path, "utf8").replace(/\r\n/g, "\n")`. Alternatively, explicitly support both line endings. Do not normalize binary data or tests whose purpose is to verify exact bytes.
- Never search raw source with LF-only boundaries such as `indexOf("\n}\n")`. Check that start and end markers were found before slicing or evaluating extracted code; otherwise CRLF can silently produce an empty snippet and misleading errors such as `... is not a function`.
- When adding or changing source-reading tests, verify them with both LF and CRLF input. For UI tests, run the suite against a temporary copy with the UI source files converted to CRLF; do not rewrite the working tree or change global Git settings. A passing macOS run alone does not establish Windows compatibility.
- For Windows-only CI test failures, check line-ending assumptions early. This has repeatedly affected `desktop/tests`, including the reset-inventory function-extraction tests; do not fix it by skipping Windows tests or weakening assertions.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues on the fork `avegetablechicken/Copool` (not upstream `AlickH/Copool`), using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default triage labels are used as-is (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one root `GLOSSARY.md` and `docs/adr/`, both created lazily. See `docs/agents/domain.md`.
