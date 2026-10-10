# Rust 跨平台版本

开发分支：`develop/rust-cross-platform`。

Rust 桌面版本的应用名为 `Copool`，Bundle ID / 应用标识为 `io.github.copool.rust`。

这是可以独立构建和运行的 Rust + Tauri 2 桌面版本，目标为 macOS、Windows 和 Linux。Swift/Xcode 版本仍保留；当前 Rust 版本尚未达到原版全部功能对等，不能直接替换现有发布版本。

## 架构

- `crates/copool-core`：账号导入、身份隔离、持久化、额度刷新、自动/手动切换及代理生命周期。不依赖 Tauri，可独立测试。
- `desktop`：Tauri 命令边界、单实例、系统托盘、原生窗口与本地 HTML/CSS/JavaScript 界面。前端不直接访问文件系统、启动 shell 或持有账号 OAuth token。
- `Sources/Copool/Resources/proxyd-src/src`：与已有远程 daemon 共用的 Rust 代理引擎及模型，使用 `#[path]` 引用同一份源码，避免复制后分叉。原 daemon 仍可单独构建。
- `.github/workflows/rust.yml`：macOS、Windows、Linux 的核心测试、桌面构建及独立 daemon 检查。配置已加入仓库，本机验证不等于三平台 CI 已运行。

架构参考本地 `../OpenQuota` 的 Tauri/Rust 分层和 `../coding-agent-proxy` 的 Rust workspace、轻量静态前端与托盘应用组织。没有对参考仓库的路径依赖。

## 构建与运行

需要 Rust 1.89+（推荐当前 stable）及各平台原生构建工具。

- macOS：Xcode Command Line Tools。
- Windows：Visual Studio C++ Build Tools、Windows SDK、WebView2 Runtime。
- Ubuntu 24.04：`build-essential pkg-config libssl-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`。

在仓库根目录：

```sh
cargo test -p copool-core --locked
cargo run -p copool-desktop --locked
cargo build -p copool-desktop --release --locked
```

`target/release/copool-desktop`（Windows 为 `.exe`）是可执行文件。该工作流不依赖根目录遗留 Electron `package.json`，也不需要 Node 来构建 UI。检查前端语法可使用 `node --check desktop/ui/app.js`。

独立远程 daemon：

```sh
cargo check --manifest-path Sources/Copool/Resources/proxyd-src/proxyd/Cargo.toml --locked
```

构建脚本（在仓库根目录运行）：

```sh
python3 scripts/build_rust_desktop.py            # macOS：release 版 Copool.app
python3 scripts/build_rust_desktop.py --debug    # 调试版应用包
python3 scripts/build_rust_desktop.py --target universal-apple-darwin
python3 scripts/build_rust_desktop.py --no-bundle  # 只构建可执行文件（非 macOS 默认如此）
```

脚本先运行前端 `node --check` 与 `desktop/tests`（`--skip-checks` 跳过），再用 Tauri CLI 打包；未安装 `cargo tauri` 时通过 `npx @tauri-apps/cli@^2` 调用。打包会经 `beforeBundleCommand` 构建 WidgetKit 扩展，之后先签小组件、再签主应用（ad-hoc，分别使用 `desktop/native` 下的 entitlements），并校验签名、架构和版本。

产物为 `target/<release|debug>/bundle/macos/Copool.app`（指定 `--target` 时位于 `target/<triple>/…`，路径相对仓库根目录）。此包使用正常数据目录，不含烟测账号或测试环境覆盖；最低 macOS 版本为 14.0。2026-10-10 本机以该脚本生成 arm64 release 包并通过 `codesign --verify --deep --strict`；未经公证，不是分发版本。

尚未启用发布签名、公证、自动更新或发布。macOS 原 Swift 正式发布继续遵循 `release-macos.md`。

## 已实现

- 导入本机 Codex 登录、单个/批量认证 JSON、Swift/Rust 账号文件及 Refresh Token。
- 同一工作区下按登录主体区分账号；重复导入更新认证信息。
- 账号名称和出站代理编辑、删除、切换，切换前备份原本机登录。
- 五小时/每周用量刷新、额度显示、手动智能切换与定时自动切换。
- 智能切换排除过期（超过 15 分钟）、刷新失败、额度耗尽及隐藏/停用账号。
- 本地 OpenAI 兼容代理：`/v1/models`、`/v1/responses`、`/v1/chat/completions`、SSE、模型映射、失败重试与代理请求中的 OAuth 刷新。
- 代理密钥生成/轮换、回环监听、启动/停止、打开应用时自动启动代理的设置。
- 界面采用 Coport（`../coding-agent-proxy/gui/ui`）风格：360 点宽的状态栏菜单式面板（启动时隐藏；点击状态栏图标在图标下方弹出，再次点击、失去焦点或 Esc 收起；高度随内容，最多屏幕高度的 85%；定位逻辑与 Coport 共用 `desktop/src/placement.rs`），顶部为应用名与文字链接（代理 · 设置 · 退出），子页面以“‹ 账号”返回（macOS ⌘[）；内容为带标题的浅色块、11 点系统字体、小号按钮/开关/下拉框和单色细进度条。展开时每个账号一个块，折叠时为对齐的单行列表；保留右键代理编辑。导入菜单与账号右键菜单只用原生菜单；此前已无入口的粘贴 auth.json / Refresh Token 导入对话框已移除。此分支不再追求与 SwiftUI 布局逐项一致。
- macOS 使用原生 Popover 材质（圆角 12 点）、透明 WebView 和系统语义色；Windows/Linux 使用对应的静态主题后备。
- macOS 字体保留 WebKit 系统字体的自动光学字号，不强制写入 `opsz` 数值。
- macOS 图标由本机 AppKit 按准确逻辑尺寸生成 SF Symbols 的 1×/2×/3×资源；菜单栏使用原版 figure.pool.swim 模板符号。其他平台保留矢量图标与原应用图标资源。
- 原版 11 种语言文案由 scripts/sync_rust_locales.py 同步，不另建翻译副本。
- 系统托盘入口、单实例、独立数据目录。

## 数据与迁移

新版本默认写入系统本地数据目录的 `CopoolRust` 子目录：macOS 为 `~/Library/Application Support/CopoolRust`，Windows 为 `%LOCALAPPDATA%\CopoolRust`，Linux 为 `$XDG_DATA_HOME/CopoolRust` 或 `~/.local/share/CopoolRust`。

可使用 `COPOOL_DATA_DIR` 指定独立账号库。Codex 认证文件遵循 `CODEX_HOME/auth.json` 或 `~/.codex/auth.json`；测试时可通过 `COPOOL_CODEX_AUTH_FILE` 指定独立认证文件，不必修改开发工具自身的环境目录。

启动不会自动迁移或修改 Swift 数据。在“导入账号”中选择原版 `~/Library/Application Support/CodexToolsSwift/accounts.json`。导入逐项验证，整批验证成功后才写入；重复账号保留本地 ID。账号附加字段保留，但原版全局设置、Sub2api 缓存和工作区目录不会迁移，原文件不会修改。

原版 `settings.json` 中独立保存的 `accountProxyURLs` 需要在新版本中逐账号配置。登录凭据保存在本地账号库，Unix 权限为 `0600`；不是系统钥匙串加密存储。Windows 使用当前用户数据目录及其 ACL，不额外宣称实现了独立凭据 ACL。

“切换账号”是唯一正常账号操作中会替换当前 Codex 登录文件的操作；上一份登录保存为 `auth-before-switch.json`。删除账号不删除当前 Codex 登录。损坏的账号文件会报错并保留原内容，不会自动重置为空。临时文件与目标文件位于同一目录，使用跨平台原子替换。

## 尚未迁移的功能

- 内置浏览器 OAuth 登录、工作区发现与切换邀请流程；当前可通过 Codex CLI 登录后导入，或导入 auth.json 文件。
- SSH 远程节点部署、启停和日志页面；已有独立 Rust daemon 保留。
- Cloudflared 快速/命名隧道、Cloudflare DNS 管理。
- Sub2api provider、远程账号同步和缓存。
- 编辑器重启、OpenCode 同步、系统登录时自启动。新增的迁移提示目前只提供中英文，原版文案已复用全部语言。
- Apple WidgetKit 小组件和 iOS 客户端；本版本跨平台范围为桌面。

## 验证范围

`python3 scripts/audit_rust_test_migration.py --require-complete` 盘点原版 Swift 测试：338 项已移植，12 项随 Coport 风格面板退役（542 点网格与 iOS 列数、用量圆环的圆点阈值、玻璃凹槽配色），每项原因记录在脚本中，不计为已移植。工具栏、细进度条、折叠行悬停、公网访问模式与状态、远程服务器详情与日志由同一组展示模型渲染并通过对应的原版测试。用量回归测试 `crates/copool-core/src/usage_regression_tests.rs` 覆盖原版请求头、`/api/codex/usage` 的重置次数路径、默认官方地址及回环 `chatgpt_base_url`。

核心测试使用临时目录和合成凭据，覆盖同工作区多主体、重复导入、批量失败回滚、损坏文件保护、Swift 附加字段、账号选择、文件替换与权限、代理鉴权/密钥轮换/停止以及原代理引擎的请求转换和 SSE 回归测试。

本机验证 macOS 编译、45 项核心测试及回环代理行为，并使用临时目录和合成账号检查原生窗口、展开/折叠卡片、代理页、设置页和 Retina 图标。Windows/Linux 原生构建需通过三平台 CI 或对应机器验证。未使用真实账号验证在线 OAuth、额度接口或上游付费请求，也未发布此版本。

2026-10-09 补跑原版 Swift 套件：332 项 XCTest 与 5 项 Swift Testing 全部通过。首次在 Documents 内运行时有本地化资源访问权限和英文断言的语言环境问题；将原样源码复制到临时目录并指定 `-testLanguage en -testRegion US` 后通过，未修改测试源码。这是 Swift 基线验证，不代表 Rust 已通过原版所有行为的等价测试。
