# Copool Repository Rules

Copool is a Rust/Tauri desktop app: `crates/copool-core` (accounts, usage, proxy, remote servers), `desktop` (Tauri shell, `desktop/ui` web UI, `desktop/widget` native macOS WidgetKit extension) and `resources` (shared proxy engine and daemon, prebuilt daemons, localization).

## Local App Bundle Build

- For requests to build the application locally, run from the repository root:

```bash
python3 scripts/build_rust_desktop.py > /tmp/copool-app-bundle-build.log 2>&1
```

- `scripts/build_rust_desktop.py` is the one build entry point on every platform: it parses the options, checks the UI and hands over to its platform's module (`macos_build.py`, `windows_build.py`, `linux_build.py`; shared steps in `build_common.py`). It only builds: it never installs, replaces or restarts an app. Change a platform's build in its module, and a machine's own Linux build (a GUI SDK environment, how it installs builds) only through `linux_build.py`'s hooks; do not add another build script.
- On macOS this checks the UI, builds a Release `Copool.app` with the Tauri CLI (its pre-bundle hook builds the WidgetKit extension from `desktop/widget/CopoolWidgets.xcodeproj`), signs the widget and then the app with the entitlements in `desktop/native`, and verifies the signature, architecture and version. Signing uses the Apple Development certificate in the login keychain and the app group `<Team ID>.io.github.copool.rust`, else ad hoc with `group.io.github.copool.rust` (`scripts/macos_signing.py`; `COPOOL_SIGN_IDENTITY` overrides, `-` for ad hoc); the build's final line reports which. On Windows it builds an unsigned NSIS installer; on Linux the executable, whose libraries it checks with `ldd`.
- Xcode and the Tauri CLI may require execution outside the sandbox to access system build services and caches; use the execution tool's approval mechanism when needed.
- Wait for a successful exit before delivering; on failure, inspect `/tmp/copool-app-bundle-build.log`. Deliver `target/release/bundle/macos/Copool.app` as a clickable absolute path, and report the verified architectures and version instead of assuming them.
- This is a locally signed build, not a notarized distribution release. An ordinary build does not include installing, launching, replacing an installed app, committing, uploading, or publishing. Perform those only when requested.
- `scripts/restart_copool.py` is the one restart entry point: it stops only the Copool processes whose executable lies in the copy it is given (`--app`: `Copool.app` on macOS, `Copool.exe` or its directory on Windows, the executable on Linux, where it defaults to `linux_build.installation()`), waits for them to exit and starts that copy again. On Linux it starts Copool in the desktop session (display, Xauthority, session bus) of the copy it stops, and refuses before stopping anything when there is no display; `--install [BUILD]` first installs a build through `linux_build.install` (refused where nothing is installed). Restarting and installing are never part of a build; run it against the user's copy only when asked, and test it against a throwaway copy (`--dry-run` shows what it would do).

## Development

- Test with `python3 scripts/test_rust.py` (formatting, `cargo test -p copool-core --locked`, UI syntax and `desktop/tests`, the standalone daemon), or the individual commands; build with `cargo build -p copool-desktop --locked`.
- The core and packaged daemon share Rust sources in `resources/proxyd-src/src`. After modifying these sources, also check the standalone daemon with `cargo check --manifest-path resources/proxyd-src/proxyd/Cargo.toml --locked`.
- Localized strings live in `resources/locales/*.lproj/Localizable.strings`; regenerate `desktop/ui/locales.js` with `python3 scripts/sync_rust_locales.py` after editing them.
- The app icon's source is the Icon Composer document `desktop/icons/Copool.icon`; regenerate the bundled PNG, ICNS and ICO and the README's `Copool.png` with `python3 scripts/build_app_icon.py` (macOS with Xcode) after editing it or the background stops in that script.
- Use temporary `COPOOL_DATA_DIR` and `COPOOL_CODEX_AUTH_FILE` paths for interactive smoke tests. Never import or switch the user's real credentials as part of testing.
- Keep limitations and platform validation status current in `docs/rust-cross-platform.md`.

- Plan tags are colored by tier so that comparable plans of every pool match: pink for the entry subscription, orange for those of $100 to $300, which their labels tell apart by multiplier, purple for Codex's $500 Pro 50x. `crates/copool-core/src/plan_tags.rs` alone names and colors plan tags and chooses workspace tags: `Application::accounts` gives each account its `planTag` and `workspaceTag`, which the UI only displays, and the widget snapshots are built from the same function. Do not derive plans in `desktop/ui` or `widgets.rs`.
- `desktop/ui/card-view.js` alone decides what an account card shows: its tags, usage rows, footer and tooltips, details, and which actions it offers as buttons and in its menu. Each function takes the card's account and an explicit context (`cardContext` in `app.js`) and returns plain data, which `card-face.js` paints on the card's front and `card-back.js` on its back; time-dependent tooltips are recomputed through its exported functions as the pointer enters. Do not decide card content per pool in the painting modules or `app.js`; test card rules by requiring card-view.js (`desktop/tests/card-view.test.cjs`).
- `desktop/ui` logic a test reaches lives in a module that loads both as a page script and through `require`, as `controllers.js` does: pure rules in `presentation.js`, `card-view.js` or `controllers.js`, and DOM-building parts (`menus.js`, `card-face.js`, `card-back.js`, `keys.js`) created with a host object that hands them `el`, the controller and the app's actions, read when used. `app.js` and `features.js` only create them, wire events and run animations. Tests require the module and pass fakes (`desktop/tests/fake-dom.cjs` for nodes); they never read `app.js` or `features.js` as source.
- What each card has under way is held by its pool's `Accounts` controller in `desktop/ui/controllers.js` alone: its refresh, its hi (sent, then received) and the refreshes that follow it until its 5h window shows, its renewal and the moment it shows as done, and which pool's windows its model switch shows. `cardState(id)` gives it in card-view.js's `ui(id)` shape; `app.js` reads it there and only shows the notices. The controller also holds the card turned over to edit it (`edit`, `cancelTurn`, `close`, `stopEditing`): one at a time, a later turn superseding one under way, and its draft (proxy, greeting schedule, removal confirmation), which it reads and saves; `card-back.js` renders the back and runs the turns through `app.js`'s flip animation. Do not add per-card task or editing state as globals in `app.js`; test it through the controller with a fake `invoke` and `clock`.
- `crates/copool-core/src/refresh_plan.rs` alone decides which cards a background round and the asks for cards speeding up refresh, and when each card is next refreshed, from the same facts, so a card's shown time is the round that asks for it. A pool supplies only its facts (the provider its tool uses, Claude's rate limit pauses); do not filter rounds or compute `next_refresh_at` in a pool's own code. Each listed card carries `refresh` (`key`, the key its own refresh switches are saved under, and `seconds`, its usual interval), which the UI reads instead of parsing card ids or deriving intervals.
- `crates/copool-core/src/login_store.rs` keeps the accounts of a pool whose tool has one Live login: loading and saving its file, following the Live login, imports, delete and edit (a deactivated account stays listed, then hidden once deleted), the switch's write, and renewal (taking the tool's renewal first, writing Copool's back, never renewing the account in use while its Live login cannot be read). Codex (`codex_pool.rs`, `codex-accounts.json`), Claude (`claude.rs`, `claude-pool.json`; its vault and Claude Code's refresh lock as the store's hooks), Grok and Antigravity supply only their facts as a `LoginPool`; their switch effects, usage requests and providers stay theirs. Its locking rule holds for every pool: the Live login is read and written outside the store's lock, and a read taken before a switch is not followed. Codex's `accounts.json` and Claude's `claude-accounts.json` are only migrated from, once, and left as they were; nothing else reads them. Do not add another copy of these flows to a pool. Test them through `login_store::suite`, which each pool runs with its fixture.
- A pool's local API proxy never reads or writes the pool's store: it asks the pool for its candidates (`provider_candidates`) and renewals (`renew_candidate`), and the pool's refresh plan alone refreshes usage.
- A third-party agent a switch syncs (OpenCode, pi, omp) is one row of `SyncAgent` in `crates/copool-core/src/platform/agents.rs`, which a switch reaches through `agents::sync` alone: the pools it takes and their providers, the settings.json key of its switch, where its logins are written (`locate`) and how (`write`, always off the async threads). Switches, the settings' agent rows and the switch results loop over `SyncAgent::ALL`; preferences (`agentSync`) and results (`SwitchExecution::agent_sync`) are keyed by agent. Do not add per-agent fields or branches elsewhere; the UI keeps only each agent's notice strings (`agentSyncNotice` in `presentation.js`) and its static row in `index.html`. Test switch gating through `sync_to_agents` with temporary targets, never the real home.

## Windows Line Endings (Recurring CI Failure)

- Windows checkouts may use CRLF (`\r\n`). Do not assume files read from disk contain only LF (`\n`), even when tests pass on macOS or Linux.
- Tests and scripts that extract source functions, match multiline text, or compare text snapshots must normalize line endings before parsing or asserting, for example `fs.readFileSync(path, "utf8").replace(/\r\n/g, "\n")`. Alternatively, explicitly support both line endings. Do not normalize binary data or tests whose purpose is to verify exact bytes.
- Never search raw source with LF-only boundaries such as `indexOf("\n}\n")`. Check that start and end markers were found before slicing or evaluating extracted code; otherwise CRLF can silently produce an empty snippet and misleading errors such as `... is not a function`.
- When adding or changing source-reading tests, verify them with both LF and CRLF input. For UI tests, run the suite against a temporary copy with the UI source files converted to CRLF; do not rewrite the working tree or change global Git settings. A passing macOS run alone does not establish Windows compatibility.
- For Windows-only CI test failures, check line-ending assumptions early. This repeatedly broke `desktop/tests` while they extracted functions from `app.js`; do not fix it by skipping Windows tests or weakening assertions.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues on the fork `avegetablechicken/Copool` (not upstream `AlickH/Copool`), using the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default triage labels are used as-is (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one root `GLOSSARY.md` and `docs/adr/`, both created lazily. See `docs/agents/domain.md`.
