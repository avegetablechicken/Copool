# Rust 跨平台版本

Rust 桌面版本的应用名为 `Copool`，Bundle ID / 应用标识为 `io.github.copool.rust`。

Copool 是 Rust + Tauri 2 桌面应用，目标为 macOS、Windows 和 Linux。原 SwiftUI 版本已从仓库移除，界面与交互仍以它为准，源码可在提交 `cae2fce` 中查阅（如 `git show cae2fce:Sources/Copool/Features/Accounts/AccountsPageModel.swift`）。

## 架构

- `crates/copool-core`：账号导入、身份隔离、持久化、额度刷新、自动/手动切换及代理生命周期。不依赖 Tauri，可独立测试。
- `desktop`：Tauri 命令边界、单实例、系统托盘、原生窗口与本地 HTML/CSS/JavaScript 界面。前端不直接访问文件系统、启动 shell 或持有账号 OAuth token。
- `desktop/widget`：macOS WidgetKit 小组件（SwiftUI），由只含小组件 target 的 `CopoolWidgets.xcodeproj` 构建并打包进 `Copool.app`。
- `resources/proxyd-src/src`：与远程 daemon 共用的 Rust 代理引擎及模型，使用 `#[path]` 引用同一份源码，避免复制后分叉。daemon 可单独构建；`resources/proxyd-prebuilt*` 为远程部署用的 Linux 预编译版本。
- `resources/locales`：11 种语言的 `Localizable.strings`，由 `scripts/sync_rust_locales.py` 生成 `desktop/ui/locales.js`。
- `.github/workflows/rust.yml`：macOS、Windows、Linux 的核心测试、桌面构建及独立 daemon 检查。配置已加入仓库，本机验证不等于三平台 CI 已运行。

架构参考本地 `../OpenQuota` 的 Tauri/Rust 分层和 `../coding-agent-proxy` 的 Rust workspace、轻量静态前端与托盘应用组织。没有对参考仓库的路径依赖。

## 构建与运行

需要 Rust 1.89+（推荐当前 stable）及各平台原生构建工具。

- macOS：Xcode（构建 WidgetKit 小组件）。
- Windows：Visual Studio C++ Build Tools、Windows SDK、WebView2 Runtime。
- Ubuntu 24.04：`build-essential pkg-config libssl-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`。

在仓库根目录：

```sh
cargo test -p copool-core --locked
cargo run -p copool-desktop --locked
cargo build -p copool-desktop --release --locked
```

`target/release/copool-desktop`（Windows 为 `.exe`）是可执行文件。构建 UI 不需要 Node。检查前端语法可使用 `node --check desktop/ui/app.js`。

独立远程 daemon：

```sh
cargo check --manifest-path resources/proxyd-src/proxyd/Cargo.toml --locked
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

尚未启用发布签名、公证、自动更新或发布。

## 已实现

- 导入本机 Codex 登录、单个/批量认证 JSON、Swift/Rust 账号文件及 Refresh Token。
- 同一工作区下按登录主体区分账号；重复导入更新认证信息。
- 账号名称和出站代理编辑、删除、切换，切换前备份原本机登录。
- 五小时/每周用量刷新、额度显示、手动智能切换与定时自动切换。
- 手动智能切换与原版一致：在可见账号中取剩余额度最高者，并列时保留当前账号；已是最佳时提示“当前账号已是最佳可用账号”，无可选账号时提示“暂无可切换账号”。定时自动切换仍排除过期（超过 15 分钟）、刷新失败、额度耗尽及隐藏/停用账号。
- 交互状态与原版一致，不使用面板级锁：导入、添加（等待浏览器登录时显示可取消的对话框）、切换（只禁用该卡片的切换和智能切换）、待授权工作区（该行变为“取消”）、代理 loading、远程卡片的保存/删除/采用实例（只锁该卡片）各自独立。删除账号保留二次确认（原版没有，按需保留）。
- 切换提示按原版 `buildSwitchNotice` 拼接：切换结果、OpenCode 同步、编辑器重启、启动 Codex 的结果由 core 的 `SwitchExecution` 返回；导入、删除、刷新、设置、代理、隧道、远程操作的提示与自动消失时间（3 秒，错误 5 秒）使用原版文案键。
- 代理页“可用账号”取代理自身的可路由候选数（`ApiProxyStatus.availableAccounts`，冷却中的账号不计），与原版 `currentCandidates().count` 一致，而非界面卡片数。
- 本地 OpenAI 兼容代理：`/v1/models`、`/v1/responses`、`/v1/chat/completions`、SSE、模型映射、失败重试与代理请求中的 OAuth 刷新。
- 代理密钥生成/轮换、回环监听、启动/停止、打开应用时自动启动代理的设置。
- 界面采用 Coport（`../coding-agent-proxy/gui/ui`）风格：360 点宽的状态栏菜单式面板（启动时隐藏；点击状态栏图标在图标下方弹出，再次点击、失去焦点或 Esc 收起；高度随内容，最多屏幕高度的 85%；定位逻辑与 Coport 共用 `desktop/src/placement.rs`），顶部为应用名与文字链接（代理 · 设置 · 退出），子页面以“‹ 账号”返回（macOS ⌘[）；内容为带标题的浅色块、11 点系统字体、小号按钮/开关/下拉框和单色细进度条。展开时每个账号一个块，折叠时为对齐的单行列表；账号代理通过卡片上的地球按钮编辑，卡片没有右键菜单。导入菜单为 Coport 式面板内菜单（原生 Liquid Glass 材质）；此前已无入口的粘贴 auth.json / Refresh Token 导入对话框已移除。此分支不再追求与 SwiftUI 布局逐项一致。
- macOS 使用原生 Popover 材质（圆角 12 点）、透明 WebView 和系统语义色；Windows/Linux 使用对应的静态主题后备。
- macOS 字体保留 WebKit 系统字体的自动光学字号，不强制写入 `opsz` 数值。
- macOS 图标由本机 AppKit 按准确逻辑尺寸生成 SF Symbols 的 1×/2×/3×资源；菜单栏使用原版 figure.pool.swim 模板符号。其他平台保留矢量图标与原应用图标资源。
- 原版 11 种语言文案由 scripts/sync_rust_locales.py 同步，不另建翻译副本。
- 系统托盘入口、单实例、独立数据目录。
- 设置“导入新账号后立即请求上游”（默认关闭）：关闭时，池中原本没有的新账号（Codex 与 Claude 的导入当前登录、浏览器登录、文件或 Refresh Token 导入）标记为“待首次刷新”（Codex 存于账号的 `firstRequestPending`，Claude 为 `firstRequestPending` 字段）。在手动刷新该卡片或“刷新全部”之前，导入时的额度、工作区名称、Claude 账号资料查询，后台定时刷新，待授权工作区目录刷新，以及本地 API 代理的路由与候选额度刷新都跳过它，以便先设置账号代理；后台刷新不会放行。重新导入或重新登录仍在等待的账号同样不联网；已放行的账号沿用其代理。登录换令牌与 Refresh Token 兑换属于导入本身，改为经系统代理。Claude 浏览器添加时如未查询资料，首次刷新时经账号代理补查套餐；套餐未知的 Claude 卡片不显示套餐标签。Claude Code 登录缺少 `oauthAccount` 时，先按令牌匹配池中账号；匹配不到则（选项关闭时，或打开但查询失败时）以“Claude Code”为名加入池中，首次手动刷新时先经账号代理查询 profile 识别账号（令牌过期则先刷新），识别为池中已有账号时合并为一张卡片并转移当前选择；切换到尚未识别的账号时 `.claude.json` 的 `oauthAccount` 写为 null，由 Claude Code 自行补全。刷新后写回 Claude Code 的令牌只在身份确实相同或 refresh token 相同时写入。
- Claude Code 账号池（`crates/copool-core/src/claude.rs`）：账号页顶栏 Copool 旁的 Codex / Claude 有色图标切换显示两个池（⌘1 / ⌘2，选择保存在界面本地存储）。Claude 池支持导入当前 Claude Code 登录、浏览器 OAuth 添加账号（claude.ai，Claude Code 的公开客户端，回环端口随机）、`/api/oauth/usage` 的 5 小时与每周额度、令牌过期或被拒时刷新、账号代理、删除、切换、智能切换，以及沿用“自动智能切换”设置的定时自动切换。卡片沿用 Codex 卡片布局，不显示 credits/重置次数；Max 套餐显示倍数（如 MAX 20X）。

## 数据与迁移

新版本默认写入系统本地数据目录的 `CopoolRust` 子目录：macOS 为 `~/Library/Application Support/CopoolRust`，Windows 为 `%LOCALAPPDATA%\CopoolRust`，Linux 为 `$XDG_DATA_HOME/CopoolRust` 或 `~/.local/share/CopoolRust`。

Claude 池的“导入 → 导入 JSON 文件”使用系统文件选择器，支持任意位置的 Claude Code 凭据 JSON（`claudeAiOauth` 包装或直接的令牌对象，可携带 `oauthAccount`），包括从 macOS 钥匙串导出的 JSON。macOS、Windows、Linux 共用导入逻辑；导入不自动切换账号，重复凭据更新已有记录，无身份信息时遵循新账号延迟请求设置，首次刷新补全身份。原有“导入当前 Claude Code 登录”仍从钥匙串或默认凭据 JSON 文件读取。文件入口、JSON 校验与切换使用合成凭据验证；Windows/Linux 原生文件选择器和真实钥匙串导出仍待对应平台验证。

Claude 池保存在数据目录的 `claude-accounts.json`。 登录及导入时的 OAuth profile 查询先按账号/组织身份或已有 access/refresh token 匹配池中账号，复用其账号代理；无法识别的账号使用系统/环境代理，不借用当前账号的代理。此路径已用隔离目录和模拟网络测试，未用真实账号验证。Claude Code 的当前登录：macOS 为钥匙串 `Claude Code-credentials`（设置 `CLAUDE_CONFIG_DIR` 时为按目录哈希的条目，经 `/usr/bin/security` 读写，写入走标准输入），其他平台为 `~/.claude/.credentials.json`（或 `$CLAUDE_CONFIG_DIR/.credentials.json`）；账号身份为 `~/.claude.json`（或 `$CLAUDE_CONFIG_DIR/.claude.json`）中的 `oauthAccount`。当前账号跟随 Claude Code 的登录，并采用它刷新后的令牌；Copool 为当前账号刷新的令牌会写回 Claude Code。切换只替换凭据中的 `claudeAiOauth`（保留 MCP 等其他条目）和 `.claude.json` 的 `oauthAccount` 成员（其余内容逐字节保留），切换前的登录保存为 `claude-before-switch.json`，失败时回滚。这与 cc-account-switcher 的做法一致（钥匙串 `-a $USER` 或 0600 的凭据文件，加 `oauthAccount`）；`oauthAccount` 只在 `.claude.json` 中没有、而配置目录内的 `.claude.json` 有时改写后者。切换提示要求重启 Claude Code；若 `~/.claude/settings.json` 中有 `apiKeyHelper` 或 `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_BASE_URL` / `CLAUDE_CODE_OAUTH_TOKEN` / `CLAUDE_CODE_USE_BEDROCK|VERTEX|FOUNDRY`（cc-switch 切换第三方供应商时写入的键），Claude Code 不会使用账号登录，切换提示会以错误样式列出这些键，Copool 不修改这些由其他工具写入的键。
- Claude 池的 Sub2api（`crates/copool-core/src/claude/sub2api.rs`）：设置页“非官方 Provider”中，经“添加 Sub2api Provider”菜单选 Claude 后与 Codex 一样填写名称、可选管理 API 地址、管理员账号与密码及 TLS 开关；保存时从 Claude Code `settings.json` 读取网关及匹配网关的 API Key，管理地址留空则从网关推导，已有 Provider 的网关和钥匙串 Key 保持兼容（两类 Provider 同在一个区块，行首以 Codex / Claude 有色图标区分；密码与 Key 存系统钥匙串，Provider 列表存 `claude-accounts.json`）。导入菜单“同步非官方 Provider”以管理员接口列出 `platform=anthropic`、`type=oauth`、`status=active` 的账号（与 OpenQuota 相同），额度取 `/api/v1/admin/accounts/{id}/usage` 的 `five_hour` / `seven_day`，套餐取 `subscription_tier`；Sub2api 对空闲 5 小时窗口返回的 0% 且无重置时间视为无数据。切换到 Sub2api 卡片与 cc-switch 切第三方供应商相同：在 `~/.claude/settings.json` 的 `env` 写入 `ANTHROPIC_BASE_URL`（网关）与 `ANTHROPIC_AUTH_TOKEN`（Key），并移除会与之冲突的 `ANTHROPIC_API_KEY`，只改写 `env` 成员、其余内容逐字节保留，切换前的 `env` 存入 `claude-before-switch.json`。同一 Provider 的卡片共用网关，其网关生效时这些卡片都为当前、官方账号不为当前（与 Codex 池的 Sub2api 一致）。切回官方账号时只移除网关属于已配置 Provider 的这两个键。测试时可用 `COPOOL_CLAUDE_CONFIG_DIR` 指定独立目录（只用其中的文件，不访问钥匙串）。

可使用 `COPOOL_DATA_DIR` 指定独立账号库。Codex 认证文件遵循 `CODEX_HOME/auth.json` 或 `~/.codex/auth.json`；测试时可通过 `COPOOL_CODEX_AUTH_FILE` 指定独立认证文件，不必修改开发工具自身的环境目录。

启动不会自动迁移或修改 Swift 数据。在“导入账号”中选择原版 `~/Library/Application Support/CodexToolsSwift/accounts.json`。导入逐项验证，整批验证成功后才写入；重复账号保留本地 ID。账号附加字段保留，但原版全局设置、Sub2api 缓存和工作区目录不会迁移，原文件不会修改。

原版 `settings.json` 中独立保存的 `accountProxyURLs` 需要在新版本中逐账号配置。登录凭据保存在本地账号库，Unix 权限为 `0600`；不是系统钥匙串加密存储。Windows 使用当前用户数据目录及其 ACL，不额外宣称实现了独立凭据 ACL。

“切换账号”是唯一正常账号操作中会替换当前 Codex 登录文件的操作；上一份登录保存为 `auth-before-switch.json`。删除账号不删除当前 Codex 登录。损坏的账号文件会报错并保留原内容，不会自动重置为空。临时文件与目标文件位于同一目录，使用跨平台原子替换。

## 已知差异与限制

- 浏览器 OAuth 登录与工作区授权、SSH 远程节点、Cloudflared 隧道、Sub2api、编辑器重启、OpenCode 同步和登录时自启动均已实现，但未用真实账号、服务器或 Cloudflare 帐户验证。
- 远程服务器“探测”使用已保存的配置，而原版使用卡片中未保存的草稿；先保存再探测。
- 非 macOS 托盘右键菜单（Open/Quit Copool）尚未本地化；原版 macOS 只有 MenuBarExtra 面板，没有此菜单。
- 少量新增提示（如复制成功、Sub2api 未配置）只提供中英文，其余文案复用全部 11 种语言。
- 不包含原版的 iOS 客户端；范围为桌面。
- Claude 池只管理账号与 Claude Code 登录，本地 API 代理、远程服务器同步和小组件仍只服务 Codex 池；切换后正在运行的 Claude Code 会话可能需要重启才使用新账号。Claude 的 OAuth 登录、额度与令牌刷新接口来自 Claude Code / OpenQuota 的公开实现，仅用合成数据测试，未用真实账号验证。

## 验证范围

2026-10-12（Sub2api 安全存储）：macOS 的 Provider 密码和 Key 改用 Security Framework 的 SecItem 接口按 service/account 查询，避免旧 keyring 后端先解析默认钥匙串文件的路径依赖；保持原 service/account，继续使用系统钥匙串。只有条目不存在才返回空值，存储不可用、锁定和授权失败仍报错，空账号标识在访问前拒绝。Windows/Linux 后端不变。本机 `python3 scripts/test_rust.py --offline` 通过（328 项核心测试、120 项前端测试、格式和独立 daemon 检查），桌面 `cargo check -p copool-desktop --locked --offline` 通过；未访问真实 Provider 凭据，真实钥匙串兼容性及刷新恢复仍待应用运行验证。

2026-10-12（Claude JSON 文件导入）：本机 macOS 上 `python3 scripts/test_rust.py --offline` 通过（326 项核心测试、120 项前端测试、JavaScript 语法、Rust 格式、独立 daemon 检查）；`cargo check -p copool-desktop --locked --offline` 通过。新增测试覆盖凭据 JSON 校验、重复导入、延迟请求、身份补全、导入后文件切换以及 Claude/Codex 文件入口隔离和取消行为。测试使用临时目录及模拟网络；Windows/Linux 原生验证和真实账号验证尚未执行。

2026-10-12：最近一次 GitHub Actions（运行 `38168314678`）中 macOS/Linux 的测试及桌面构建通过，Windows 在字体测试中因 CRLF 换行误报 `body rule is missing`，尚未执行桌面构建。已修正匹配并覆盖 LF/CRLF 两种格式；本机 macOS 验证 323 项核心测试、117 项前端测试、JavaScript 语法、Rust 格式与独立 daemon 检查通过。修复后的 Windows 完整 CI 仍待运行。

原版 Swift 测试在移除前已全部盘点：337 项以原测试名（`Suite.testName`）移植到 `crates/copool-core` 与 `desktop/tests`，13 项随 Coport 风格面板退役（542 点网格与 iOS 列数、用量圆环的圆点阈值、玻璃凹槽配色、工具栏用量显示切换——只在设置页切换）。工具栏、细进度条、折叠行悬停、公网访问模式与状态、远程服务器详情与日志由同一组展示模型渲染并通过对应的原版测试。用量窗口按 OpenQuota 的规则按时长识别：只显示 5h（18000 秒）与周额度（604800 秒），无时长的窗口按位置兜底，其他时长（如免费计划的 30 天）不显示；只有每周限额的账号 5h 显示“无数据 / 重置时间不可用”，不再借用每周或 Codex Spark 的窗口；排序时把没有 5h 限额视为 5h 无约束。用量回归测试 `crates/copool-core/src/usage_regression_tests.rs` 覆盖原版请求头、`/api/codex/usage` 的重置次数路径、默认官方地址及回环 `chatgpt_base_url`。

核心测试使用临时目录和合成凭据，覆盖同工作区多主体、重复导入、批量失败回滚、损坏文件保护、Swift 附加字段、账号选择、文件替换与权限、代理鉴权/密钥轮换/停止以及原代理引擎的请求转换和 SSE 回归测试。

本机验证 macOS 编译、45 项核心测试及回环代理行为，并使用临时目录和合成账号检查原生窗口、展开/折叠卡片、代理页、设置页和 Retina 图标。Windows/Linux 原生构建需通过三平台 CI 或对应机器验证。未使用真实账号验证在线 OAuth、额度接口或上游付费请求，也未发布此版本。

移除前（2026-10-09）原版 Swift 套件 332 项 XCTest 与 5 项 Swift Testing 全部通过；这是原版基线，不代表 Rust 已通过原版所有行为的等价测试。

2026-10-12（Claude 刷新诊断与限流退避）：Claude profile、usage 与令牌刷新保留 reqwest 底层错误链，证书不受信任等错误不再仅显示 `error sending request`。usage 返回 HTTP 429 后，将该账号的重试时间持久化并冷却 5 分钟；手动与后台刷新均遵守，冷却后允许重试。这是本地固定退避策略，尚未解析服务端 Retry-After，也不保证 5 分钟后上游解除限流。本机 macOS 的 19 项 Claude 模块测试通过（模拟凭据及临时本机 HTTP 服务），覆盖冷却、到期恢复与错误链。无凭据 Rust 连通性探测中，本机 7893 到 profile/usage 均返回预期的 401；8787 均因 `UnknownIssuer` 失败。未绕过 TLS 校验，未修改系统证书信任；真实账号刷新及 Windows/Linux 尚未验证。

2026-10-12（macOS 本地代理 TLS 信任修复）：共享账号 HTTP 客户端在 macOS 显式使用 reqwest native-tls，通过 Security Framework 遵循钥匙串证书信任设置，修复 Coport 8787 代理访问 Claude 时 Rustls 返回 `UnknownIssuer` 的问题；核心和独立 daemon 均加入仅 macOS 启用的依赖并更新锁文件。保留证书与主机名校验，未修改系统信任；Windows/Linux 继续使用 Rustls，尚未在对应平台验证。该次修复尚保留 Codex 本地 `chatgpt_base_url` 的回环 HTTP 兼容行为；后续已移除，见下文。本机通过实际共享客户端、不携带凭据访问 8787 的 Claude profile/usage，均收到预期 HTTP 401，确认不再卡在 TLS 验证；真实账号额度刷新未验证。330 项核心测试、120 项前端测试、Rust 格式、UI 语法、桌面和独立 daemon 编译检查通过。核心网络测试与连通性探测需在沙盒外运行（沙盒禁止本机端口操作）。本次未打包或替换已安装应用。

2026-10-12（尊重 Codex 配置协议）：移除回环 HTTPS 地址自动改写为 HTTP 的兼容处理。额度、工作区与代理上游请求均保留 `chatgpt_base_url` 的协议，HTTPS 使用正常证书校验；本地地址不走账号代理的规则保留。回归测试覆盖 HTTP/HTTPS 与 IPv4、localhost、IPv6，以及共享配置的 HTTPS 地址保持一致。macOS 的 328 项核心测试、Rust 格式检查与独立 daemon 编译检查通过；删除的两项测试只覆盖已移除的协议改写函数。Windows/Linux 未实机验证。

2026-10-12（Sub2api 设置表单对齐）：Claude 与 Codex 均使用名称、可选管理 API 地址、管理员账号、密码和 TLS 开关。Claude 在保存时读取当前 `settings.json` 的网关与匹配的 Key，分别保存管理地址和网关；配置多个 Claude Provider 时需先在 Claude Code 配好对应网关，已有 Provider 不会复制其他网关的 Key。旧配置及钥匙串凭据兼容。macOS 上 329 项核心测试、120 项前端测试、Rust 格式、UI 语法及独立 daemon 检查通过；核心网络测试因本机端口限制在沙盒外重跑通过。使用临时目录及合成凭据，未用真实 Provider、Windows/Linux 或图形界面验证。
