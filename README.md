# Copool

<p align="center">
  <img src="./Copool.png" alt="Copool Icon" width="160" />
</p>

Copool is a cross-platform desktop app (macOS, Windows, Linux) that manages Codex/ChatGPT auth accounts, usage-based smart switching, and local/remote API proxy workflows. It is built with a Rust core and a Tauri 2 shell.

Copool 是一个跨平台桌面应用（macOS、Windows、Linux），用于管理 Codex/ChatGPT 授权账号、按用量智能切换，以及本地/远程 API 代理工作流。采用 Rust 核心与 Tauri 2 外壳。

## Features / 功能

- Account import/switch/delete and usage refresh (5h / 1week)
- 账号导入/切换/删除与用量刷新（5h / 1week）
- Import and refresh Sub2api OpenAI/Codex upstream accounts
- 导入并刷新 Sub2api 的 OpenAI/Codex 上游账号
- Smart switch based on remaining quota score, manual or automatic
- 基于剩余额度评分的智能切换，可手动或自动
- Local OpenAI-compatible API proxy with model compatibility mapping
- 本地 OpenAI 兼容 API 代理与模型兼容映射
- Cloudflared public tunnel management
- Cloudflared 公网隧道管理
- Remote Linux deployment/start/stop/logs for proxy nodes over SSH
- 远程 Linux 代理节点 SSH 部署/启停/日志
- ChatGPT OAuth sign-in, plus editor restart / Codex launch / OpenCode sync on account switch
- ChatGPT OAuth 登录，以及切换账号时的编辑器重启 / 拉起 Codex / 同步 OpenCode
- Status bar panel, and a WidgetKit widget on macOS
- 状态栏面板，macOS 上另有 WidgetKit 小组件

## Requirements / 环境要求

- Rust 1.89+ (current stable recommended)
- macOS 14+ with Xcode (for the WidgetKit extension)
- Windows: Visual Studio C++ Build Tools, Windows SDK, WebView2 Runtime
- Ubuntu 24.04: `build-essential pkg-config libssl-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`

## Build & Run / 构建与运行

```bash
cargo run -p copool-desktop --locked        # run / 运行
python3 scripts/test_rust.py                # tests / 测试
python3 scripts/build_rust_desktop.py       # macOS: ad-hoc-signed Copool.app; elsewhere the executable
```

See [docs/rust-cross-platform.md](docs/rust-cross-platform.md) for details, data locations and validation status.

详细说明、数据目录与验证情况见 [docs/rust-cross-platform.md](docs/rust-cross-platform.md)。

## Project Structure / 项目结构

- `crates/copool-core`: accounts, usage, switching, proxy, tunnel, remote servers and settings
- `desktop`: Tauri shell (`src`), web UI (`ui`), UI tests (`tests`), macOS widget (`widget`)
- `resources/proxyd-src`: proxy engine shared by the core and the standalone remote daemon
- `resources/proxyd-prebuilt*`: prebuilt Linux daemons for remote deployment
- `resources/locales`: localized strings (11 languages)
- `scripts`: build, test and localization tooling

## Reference Project / 参考项目

- [170-carry/codex-tools](https://github.com/170-carry/codex-tools)

Copool began as a Swift-native migration and redesign of the original Tauri-based implementation, and has since moved to a cross-platform Rust/Tauri app that keeps the SwiftUI app's interface and behavior.

Copool 最初是对原 Tauri 版本的 Swift 原生迁移与重构，现已迁移为保留 SwiftUI 版界面与交互的跨平台 Rust/Tauri 应用。

## Acknowledgements / 致谢

- Thanks to the original authors and contributors of `170-carry/codex-tools`.
- 感谢 `170-carry/codex-tools` 的原作者与贡献者。
- Thanks to all users who provided migration feedback and UI/UX suggestions.
- 感谢所有提供迁移反馈与界面建议的用户。

## License / 许可证

Please follow the upstream license and your organization’s compliance requirements when reusing code from referenced projects.

复用参考项目代码时，请遵循上游许可证与所在组织的合规要求。
