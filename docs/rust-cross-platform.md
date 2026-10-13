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
- 界面采用 Coport（`../coding-agent-proxy/gui/ui`）风格：360 点宽的状态栏菜单式面板（启动时隐藏；点击状态栏图标在图标下方弹出，再次点击、失去焦点或 Esc 收起；高度随内容，最多屏幕高度的 85%；定位逻辑与 Coport 共用 `desktop/src/placement.rs`），顶部为应用名与文字链接（代理 · 设置 · 退出），子页面以“‹ 账号”返回（macOS ⌘[）；内容为带标题的浅色块、11 点系统字体、小号按钮/开关/下拉框和单色细进度条。展开时每个账号一个块，折叠时为对齐的单行列表；账号代理通过卡片上的地球按钮编辑，卡片没有右键菜单。导入菜单为 Coport 式面板内菜单（原生 Liquid Glass 材质）；此前已无入口的粘贴 auth.json / Refresh Token 导入对话框已移除，Refresh Token 导入的后端命令随后也已删除。此分支不再追求与 SwiftUI 布局逐项一致。
- macOS 使用原生 Popover 材质（圆角 12 点）、透明 WebView 和系统语义色；Windows/Linux 使用对应的静态主题后备。
- macOS 字体保留 WebKit 系统字体的自动光学字号，不强制写入 `opsz` 数值。
- macOS 图标由本机 AppKit 按准确逻辑尺寸生成 SF Symbols 的 1×/2×/3×资源；菜单栏使用原版 figure.pool.swim 模板符号。其他平台保留矢量图标与原应用图标资源。
- 原版 11 种语言文案由 scripts/sync_rust_locales.py 同步，不另建翻译副本。
- 系统托盘入口、单实例、独立数据目录。
- 设置“导入新账号后立即请求上游”（默认关闭）：关闭时，池中原本没有的新账号（Codex 与 Claude 的导入当前登录、浏览器登录或文件导入）标记为“待首次刷新”（Codex 存于账号的 `firstRequestPending`，Claude 为 `firstRequestPending` 字段）。在手动刷新该卡片或“刷新全部”之前，导入时的额度、工作区名称、Claude 账号资料查询，后台定时刷新，待授权工作区目录刷新，以及本地 API 代理的路由与候选额度刷新都跳过它，以便先设置账号代理；后台刷新不会放行。重新导入或重新登录仍在等待的账号同样不联网；已放行的账号沿用其代理。登录换令牌属于导入本身，改为经系统代理。Claude 浏览器添加时如未查询资料，首次刷新时经账号代理补查套餐；套餐未知的 Claude 卡片不显示套餐标签。Claude Code 登录缺少 `oauthAccount` 时，先按令牌匹配池中账号；匹配不到则（选项关闭时，或打开但查询失败时）以“Claude Code”为名加入池中，首次手动刷新时先经账号代理查询 profile 识别账号（令牌过期时由 Copool 在池内续期），识别为池中已有账号时合并为一张卡片并转移当前选择；切换到尚未识别的账号时 `.claude.json` 的 `oauthAccount` 写为 null，由 Claude Code 自行补全。续期更新池内凭据；当前本机登录仍匹配且未被外部改动时无交互写回，非当前账号只更新池内副本。
- Claude Code 账号池（`crates/copool-core/src/claude.rs`）：账号页顶栏 Copool 旁的 Codex / Claude 有色图标切换显示两个池（⌘1 / ⌘2，选择保存在界面本地存储）。Claude 池支持导入当前 Claude Code 登录、浏览器 OAuth 添加账号（claude.ai，Claude Code 的公开客户端，回环端口随机）、`/api/oauth/usage` 的 5 小时与每周额度、令牌过期或被拒时在池内续期、账号代理、删除、切换、智能切换，以及沿用“自动智能切换”设置的定时自动切换。卡片沿用 Codex 卡片布局，底部显示 extra usage 额度与 banked 重置次数（见下文）；Max 套餐显示倍数（如 MAX 20X）。

## 数据与迁移

新版本默认写入系统本地数据目录的 `CopoolRust` 子目录：macOS 为 `~/Library/Application Support/CopoolRust`，Windows 为 `%LOCALAPPDATA%\CopoolRust`，Linux 为 `$XDG_DATA_HOME/CopoolRust` 或 `~/.local/share/CopoolRust`。

Claude 池的“导入 → 导入 JSON 文件”使用系统文件选择器，支持任意位置的 Claude Code 凭据 JSON（`claudeAiOauth` 包装或直接的令牌对象，可携带 `oauthAccount`），包括从 macOS 钥匙串导出的 JSON。macOS、Windows、Linux 共用导入逻辑；导入不自动切换账号，重复凭据更新已有记录，无身份信息时遵循新账号延迟请求设置，首次刷新补全身份。原有“导入当前 Claude Code 登录”仍从钥匙串或默认凭据 JSON 文件读取。文件入口、JSON 校验与切换使用合成凭据验证；Windows/Linux 原生文件选择器和真实钥匙串导出仍待对应平台验证。

Claude 池保存在数据目录的 `claude-accounts.json`。 登录及导入时的 OAuth profile 查询先按账号/组织身份或已有 access/refresh token 匹配池中账号，复用其账号代理；无法识别的账号使用系统/环境代理，不借用当前账号的代理。此路径已用隔离目录和模拟网络测试，未用真实账号验证。Claude Code 的当前登录：macOS 为钥匙串 `Claude Code-credentials`（设置 `CLAUDE_CONFIG_DIR` 时为按目录哈希的条目，读取和已有条目的更新使用原生 SecItem 接口；首次创建仅在手动切换时经 `/usr/bin/security -i` 标准输入执行），其他平台为 `~/.claude/.credentials.json`（或 `$CLAUDE_CONFIG_DIR/.credentials.json`）；账号身份为 `~/.claude.json`（或 `$CLAUDE_CONFIG_DIR/.claude.json`）中的 `oauthAccount`。当前账号跟随 Claude Code 的登录，并采用它刷新后的令牌；Copool 为账号续期后保存池内副本，当前本机登录的身份及凭据版本未变化时同时无交互写回；无法读取本机凭据时仍可用池内凭据查询额度或续期，但不会盲目写回。切换只替换凭据中的 `claudeAiOauth`（保留 MCP 等其他条目）和 `.claude.json` 的 `oauthAccount` 成员（其余内容逐字节保留），切换前的登录保存为 `claude-before-switch.json`，失败时回滚。这与 cc-account-switcher 的做法一致（钥匙串 `-a $USER` 或 0600 的凭据文件，加 `oauthAccount`）；`oauthAccount` 只在 `.claude.json` 中没有、而配置目录内的 `.claude.json` 有时改写后者。切换提示要求重启 Claude Code；主 `settings.json` 按下述原生提供商切换规则处理。
- Claude 池的 Sub2api（`crates/copool-core/src/claude/sub2api.rs`）：设置页的 Settings 输入框填写设置文件的名称或路径，管理 API 地址可选，管理员密码保存在钥匙串。含路径分隔符或以 `~` 开头的输入是路径（绝对路径，可用 `~/`；字段旁的系统文件选择器默认打开 Claude 配置目录并筛选 JSON），保存路径，名称按搜索规则反推：去掉 `.json`，再去掉 `settings` 及连接它的一个 `-`、`_` 或 `.`。其他输入是名称，按 Coport `src/claude_settings.rs` 的规则在 Claude 配置目录（默认 `~/.claude`，遵循 `CLAUDE_CONFIG_DIR`）内递归搜索：优先选择文件名同时包含 `settings` 和名称的 JSON，没有时匹配完整文件名或去掉 `.json` 的名称；不跟随符号链接，不搜索 `.git`，不把主 `settings.json` 当作源文件。搜索得到的路径不保存，每次使用时重新搜索。设置文件按 `claude --settings` 的格式读取，须在 `env` 中提供 `ANTHROPIC_BASE_URL` 及非空的 `ANTHROPIC_AUTH_TOKEN` 或 `ANTHROPIC_API_KEY`；顶层的环境变量字段不是 Claude Code 设置，不再接受。多个匹配、文件丢失或无效均报错，不从主配置或缓存密钥回退。
- 保存 Provider、同步账号和切换时重新读取设置文件，管理地址留空则从文件的 `ANTHROPIC_BASE_URL` 推导。导入通过管理接口列出 Anthropic OAuth 账号，管理接口使用管理员登录得到的 Bearer Token。模型密钥保留在设置文件，不复制管理员 Token，也不缓存模型密钥。设置文件保持不变，修改后下次切换立即生效，无需重新导入。

可使用 `COPOOL_DATA_DIR` 指定独立账号库。Codex 认证文件遵循 `CODEX_HOME/auth.json` 或 `~/.codex/auth.json`；测试时可通过 `COPOOL_CODEX_AUTH_FILE` 指定独立认证文件，不必修改开发工具自身的环境目录。

启动不会自动迁移或修改 Swift 数据。在“导入账号”中选择原版 `~/Library/Application Support/CodexToolsSwift/accounts.json`。导入逐项验证，整批验证成功后才写入；重复账号保留本地 ID。账号附加字段保留，但原版全局设置、Sub2api 缓存和工作区目录不会迁移，原文件不会修改。

原版 `settings.json` 中独立保存的 `accountProxyURLs` 需要在新版本中逐账号配置。登录凭据保存在本地账号库，Unix 权限为 `0600`；不是系统钥匙串加密存储。Windows 使用当前用户数据目录及其 ACL，不额外宣称实现了独立凭据 ACL。

“切换账号”是唯一正常账号操作中会替换当前 Codex 登录文件的操作；上一份登录保存为 `auth-before-switch.json`。删除账号不删除当前 Codex 登录。损坏的账号文件会报错并保留原内容，不会自动重置为空。临时文件与目标文件位于同一目录，使用跨平台原子替换。

### Claude 原生提供商切换

认证依据：[Sub2api 客户端配置生成器](https://github.com/Wei-Shaw/sub2api/blob/main/frontend/src/components/keys/UseKeyModal.vue) 将模型 API Key 写入 `ANTHROPIC_AUTH_TOKEN`；[模型 API 认证中间件](https://github.com/Wei-Shaw/sub2api/blob/main/backend/internal/server/middleware/api_key_auth.go) 优先读取 Bearer，并兼容 `x-api-key`。管理员登录 Token 仅用于管理接口，不能写入模型调用配置。

能使用钥匙串或凭据文件中官方登录的主 `settings.json` 都是官方配置：其中没有替代登录的设置，即非空的 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN`、云平台选择变量（`CLAUDE_CODE_USE_BEDROCK` 等，`0`/`false` 不算）、`ANTHROPIC_PROFILE`/联邦认证变量、`apiKeyHelper`、`forceLoginGatewayUrl`，以及取值为 `console`/`gateway` 的 `forceLoginMethod`。地址不限，经本机代理（如 Coport）的配置同样是官方配置。Sub2api 这类需要其他凭据的配置都通过 Provider 的设置文件提供。
- 官方 → Provider：把主 `settings.json` 当前内容原样记录到数据目录的 `claude-official-settings.json`（文件不存在也记录为不存在），覆盖上一次记录；再写入“该记录 ⊕ Provider 设置文件”的合并结果。
- Provider → Provider：以记录的官方配置（无记录时为去掉替代登录设置后的当前主配置）为底，合并新的设置文件，上一个 Provider 的设置不会残留。
- Provider → 官方：原样恢复最近一次记录（不删除记录）；没有记录时只删除替代登录的设置，其余保留。官方 → 官方不改动主配置。非官方期间对主配置的修改不会带回官方配置。
- 合并按 Claude Code 2.1.285 合并设置来源的方式（lodash `mergeWith` 加其 `settingsMergeCustomizer`，与 `claude --settings` 叠加在用户设置之上相同）：对象逐层合并、后者的值优先；数组拼接并去除重复的基本值，`fallbackModel` 与 `modelPicker` 由后者整体替换；`extraKnownMarketplaces`、`managedMcpServers` 只按条目合并。
- 主配置为官方时，与 Claude Code 当前登录一致的官方账号显示为当前；否则最近切换到的 Provider 显示为当前，从未经 Copool 切换的非官方配置不标记任何账号。切换失败时恢复主配置与官方记录。仅修改本机主设置，不修改设置文件、项目或组织托管策略，也不检查 shell 环境变量。切换后应重启 Claude 会话。

## 已知差异与限制

- Claude 当前状态反映本机用户级 `settings.json` 与登录凭据，不代表每个终端进程的最终配置；命令行 `--settings`、项目配置、托管配置和终端环境覆盖尚未纳入识别。已保存的 Cloud gateway 登录、默认联邦认证 Profile、宿主强制路由及继承的 Unix socket 不在本次用户设置切换的控制范围内。未新增历史配置存储，因此第三方覆盖掉的旧官方转发地址不会自动恢复；离开第三方时回到官方默认地址。

- 浏览器 OAuth 登录与工作区授权、SSH 远程节点、Cloudflared 隧道、Sub2api、编辑器重启、OpenCode 同步和登录时自启动均已实现，但未用真实账号、服务器或 Cloudflare 帐户验证。
- 远程服务器“探测”使用已保存的配置，而原版使用卡片中未保存的草稿；先保存再探测。
- 非 macOS 托盘右键菜单（Open/Quit Copool）尚未本地化；原版 macOS 只有 MenuBarExtra 面板，没有此菜单。
- 少量新增提示（如复制成功、Sub2api 未配置）只提供中英文，其余文案复用全部 11 种语言。
- 不包含原版的 iOS 客户端；范围为桌面。
- Claude 池只管理账号与 Claude Code 登录，本地 API 代理、远程服务器同步和小组件仍只服务 Codex 池；切换后正在运行的 Claude Code 会话可能需要重启才使用新账号。Claude 的 OAuth 登录、额度与令牌刷新接口来自 Claude Code / OpenQuota 的公开实现，仅用合成数据测试，未用真实账号验证。

## 验证范围

2026-10-12（Claude 当前认证识别）：本机 macOS 完整测试通过（332 项核心、120 项前端、Rust 格式、JavaScript 语法、独立 daemon 检查），桌面 `cargo check` 通过。新增回归覆盖应用重建后的同址代理订阅登录、真实网关认证、外部 Key 变更、API Key 认证、双重认证、同址多 Provider 及协议覆盖；使用临时目录和合成凭据，未修改真实账号配置，Windows/Linux 原生验证待运行。

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

2026-10-12（Claude 钥匙串重复授权，历史实现；当前写回策略见下文）：账号列表、额度刷新及自动切换的原生 SecItem 查询使用 `kSecUseAuthenticationUIFail`，需要授权时返回失败而不弹窗；只有主动导入当前登录、手动切换（含手动智能切换）允许授权 UI。仅条目不存在时回退到不限定 account 的查询，授权拒绝、取消及锁定不重试，也不读取可能过期的文件凭据。额度刷新完全不回写本机凭据。当前登录与非当前账号均使用池内副本续期，只保存到 Copool 的 `claude-accounts.json`；本机凭据不可读不阻止池内续期。可读取时仍跟随外部更新的令牌，旧本机凭据不会覆盖到期时间更晚的池内副本；池内刷新与导入、切换串行化，防止 Copool 自身并发重复续期。

切换已有钥匙串条目时仅通过 `SecItemUpdate` 更新 `kSecValueData`，不传入访问控制属性。首次创建仅允许手动操作，走 `/usr/bin/security -i` 标准输入，不带 `-U`，不将令牌放入进程参数，不在用户授权过程中用 15 秒超时杀掉进程；超出交互命令长度限制时明确报错。自动切换只允许无弹窗更新已有条目，不能后台启动 `security` 创建条目。该修复不会自动修复 Claude Code 上游重写导致的分区授权丢失；原生后台读取被拒后需主动导入授权。其他进程持有旧凭据、外部程序在读取后立即切换登录等跨进程竞争无法由 Copool 的进程内锁完全协调。

验证：macOS 上 `python3 scripts/test_rust.py --offline` 通过（339 项核心测试、120 项前端测试、格式、UI 语法及独立 daemon 检查），桌面 `cargo check -p copool-desktop --locked --offline` 通过。新增/更新回归覆盖后台无 UI 参数、拒绝后不重试、更新属性不含 ACL、后台不得创建、命令输入边界、当前账号过期/401 在池内续期且不写回、跟随外部续期、当前与非当前账号并发只续期一次，以及本机凭据不可读时池内仍可续期。测试使用临时目录和合成凭据，未访问真实钥匙串；实际系统授权窗口、新条目的 Claude Code 互操作及 Windows/Linux 原生行为待运行验证。未打包或替换已安装应用。

池内副本不等于独立 OAuth 授权：外部程序持有相同 refreshToken 时仍可能发生服务端轮换竞争；此模式不自动同步 Coport 的本地凭据来源，也不改变其刷新请求的路由策略。

2026-10-12（池内独立续期）：上述池内续期调整经 `cargo fmt --all -- --check` 与 `cargo test -p copool-core --locked --offline` 验证，339 项核心测试通过。核心测试因本机监听限制在沙箱外运行，仍使用临时目录及合成凭据；本次未打包、安装或用真实账号测试续期，Windows/Linux 未运行验证。

提交隔离验证：不包含工作区剩余的 Sub2api 识别修复时，暂存快照的 336 项核心测试通过；包含该修复的完整工作区此前为 339 项。

2026-10-12（外部 Claude 凭据轮换保护，后续增加活跃登录写回）：池内查询在发起续期前及保存结果前重新无交互读取本机凭据，对比 accessToken、refreshToken、expiresAt。确认同一账号在请求期间被其他程序更新时，优先同步外部凭据到账号池，丢弃本次过时结果并重新查询一次；续期请求失败也执行该检查。若重试期间再次变化，保存最新外部凭据并返回可再次刷新的错误，不无限重试。切换成其他账号时不将其令牌写入原账号；缺少可验证身份或钥匙串不可读时不猜测归属，继续保留池内行为。仍不回写钥匙串或 Claude Code 文件，池文件沿用 0600 临时文件与原子替换。该检查不构成跨进程刷新锁，不能消除检查后立即发生的外部写入或已经发出的并发请求。

验证：341 项核心测试及格式、diff 检查通过。新增回归覆盖外部轮换优先于本次续期成功/失败、401 后避免重复续期、连续变化只重试一次、外部切号不串号，以及仅过期时间变化也能识别。全部使用合成凭据及临时文件，未改动真实钥匙串；Windows/Linux、真实跨应用并发及应用打包未验证。

2026-10-12（恢复活跃凭据写回）：撤回所有账号仅保存池内的限制。续期后若本机登录仍属于该账号，且 accessToken、refreshToken、expiresAt 与请求前一致，则将新凭据同步到本机；外部轮换继续优先同步入池，外部切号不覆盖。macOS 更新已有钥匙串条目时使用后台无 UI 的 `SecItemUpdate`，只更新数据、不改 ACL；存在的凭据文件同步更新并保留其他字段。写回失败时仍保存已轮换的池内凭据并报告错误，不弹授权框。此检查仍不是跨进程原子比较交换。

恢复写回验证：342 项核心测试及格式检查通过，覆盖活跃账号续期后同步并保留其他字段、用量失败仍保存轮换令牌、非活跃账号不写回、外部更新优先及外部切号不覆盖。使用临时文件与合成凭据，真实钥匙串写回与跨应用授权尚未运行验证；未打包或安装。

2026-10-12（Claude 源配置发现与同步）：补齐 Coport 式递归搜索，源文件实时读取及完整主设置同步。测试覆盖命名优先级、歧义、缺失/损坏文件、符号链接和 `.git` 排除、顶层环境字段规范化、主配置保护、源文件不变、模型/认证变量更新，以及管理 Token 与模型密钥分离。351 项核心测试、120 项前端测试、核心编译和格式检查通过。测试使用隔离目录和本机模拟服务，未读取真实源配置密钥或执行真实模型请求。

## 账号统一代理

设置页的“账号统一代理”仅作为已导入订阅账号在 Copool 内日常请求的默认代理，包括 Codex / Claude 的额度与资料查询、凭据续期、工作区查询，以及订阅账号的本地 API 转发。单账号独立代理优先，账号未配置时使用设置页代理，两者均为空时沿用系统代理规则，不修改账号保存的代理地址。登录、OAuth 换令牌和导入不使用该统一配置。Sub2api 卡片的导入、刷新及 API 转发均不受此设置影响，仅沿用独立账号、Provider 与系统代理规则。不会设置系统代理或修改外部 Codex / Claude Code 进程的网络配置，也不改变远程 daemon 的代理。旧 importProxyUrl 配置读取时兼容，保存时迁移到 accountProxyUrl。支持带显式端口的 HTTP、HTTPS、SOCKS5、SOCKS5H，不支持代理认证；本机上游仍直连。验证使用隔离账号与模拟网络，未进行真实账号授权或 Windows/Linux 图形界面验证。

2026-10-12（Claude banked 重置次数）：usage 请求加 `?cedar_ember=1`，仅该请求使用 `claude-cli/2.1.280 (external, cli)` 的 User-Agent，即最早获得该块的版本（用户用真实账号实测：2.1.280 为 `eligible: true`，2.1.279 为 `ineligible_reason: "cli_version"`）；profile 与令牌续期保持原 User-Agent。`cedar_ember.grants` 中未过期授予的 `resets_left` 之和写入 `remaining_reset_count`，卡片底部显示“重置：N 次”；不合格或缺失时显示为未知（“重置：--”），不读取或保存可用于消耗重置的 grant id，也不提供消耗重置的操作。接口未公开，字段及版本门槛可能变化。用户用真实 Max 账号在 macOS 终端对比：新旧请求的 `five_hour`、`seven_day` 一致，新请求返回合格的 `cedar_ember`；不带 `skip_spend=1` 时 `extra_usage` 与 `spend` 仍返回（带该参数时两者为 null）。Copool 本身尚未用真实账号刷新验证；Sub2api 返回不含该块，次数保持未知。

Claude 卡片同时显示 extra usage 额度：额度指 extra usage 还能用的金额，按 Claude Code 的算法取两者较小值：`monthly_limit` 减去 `used_credits`（每月使用上限的余量），与组织预付余额。预付余额由 usage 返回 `extra_usage` 时经同一账号代理请求 `GET /api/oauth/organizations/{organizationUuid}/prepaid/credits`（`x-organization-uuid`、`anthropic-beta: oauth-2025-04-20`、`anthropic-version: 2023-06-01`）取得，读 `balance.money` 或 `balance.credits` 的 `amount_minor`/`exponent`，`amount` 为 -1 时不作为金额。无月上限或开启自动充值时对应一项不设上限，两项都不设上限显示“无限”；余额请求失败或无余额字段时为“--”，不以月上限推算。金额最低为 0。实测 Max 账号：月上限 $40.00、本月已用 $12.85、`balance.credits` 为 0（`disabled_reason: out_of_credits`），显示“额度：$0.00”。`is_enabled` 记入 `has_credits`，目前未用于显示。353 项核心测试、120 项前端测试、格式、UI 语法及独立 daemon 检查通过。

2026-10-12（Codex 令牌轮换与 Refresh Token 导入清理）：
- Codex：刷新用量前，若 `auth.json` 是同一账号、令牌不同且 `last_refresh` 不早于池内副本（池内副本无刷新时间时也采用），改用该登录请求并保存到池内；请求失败也保留。修复 Codex CLI 轮换 refresh token 后，池内旧令牌被 Coport（按已配置 auth.json 匹配刷新路由）以 403 拒绝、或被上游视为已用令牌的问题。不会采用其他账号的登录。实际排查：池内副本 10-02 签发、10-12 18:29 过期，Codex CLI 于 18:30 轮换；修复后当前账号刷新恢复。
- 非当前账号的刷新令牌只存于账号池，经 Coport 刷新时无法按 refresh token 匹配账号；需由 Coport 的 `account_fallback` 等配置处理，Copool 不对上游做假定。
- 删除界面早已没有入口的 Codex Refresh Token 导入：Tauri 命令 `import_refresh_token`、核心导入函数、共享源码中的 `exchange_refresh_token_at` 及其移植测试。原版 Swift 的 `exchangeAuth(email:refreshToken:)` 同样没有调用方。
- 验证：新增回归覆盖较旧/较新/其他账号的 auth.json 与失败时保留轮换；353 项核心测试、120 项前端测试、格式、UI 语法、独立 daemon 及桌面编译检查通过。诊断时只读比对了本机真实凭据的指纹与非敏感字段，未修改真实凭据。

2026-10-13（Claude 官方配置记录与 Provider 设置合并）：官方配置改按“没有替代登录的设置”判定；切到 Provider 时原样记录官方主配置并合并 Provider 设置文件，切回官方时原样恢复，取代整份同步源文件、按内容推断网关、写入第一方标记和强制写回官方地址。Provider 的 Settings 输入框可填名称（每次按名称搜索，不保存找到的路径）或设置文件路径（保存路径，名称按搜索规则反推）。测试覆盖替代登录设置的识别与删除、Claude Code 合并规则（深合并、数组去重拼接、整体替换与按条目合并的键、键序）、设置文件读取与校验、往返切换的原样恢复（含文件不存在）、Provider 间以记录为底、无记录时只删替代登录的设置、外部写入凭据时不标记官方账号，以及缺失设置文件时不作改动。352 项核心测试通过；使用隔离目录、合成凭据和本机模拟服务，未执行真实切换或模型请求，未打包安装。

### 账号卡片直连开关（2026-10-13）

账号卡片的代理弹窗右上角新增无文字开关，悬停提示和无障碍名称为“启用代理”，默认打开，适用于 Codex、Claude 及 Sub2api 账号。关闭后禁用代理地址输入，并绕过 Provider、账号统一代理、系统及环境变量代理；开启后继续使用输入的代理地址，留空沿用原有继承规则。直连状态使用现有代理字段中的 `direct://` 标记持久化，共享 HTTP 客户端明确禁用代理，独立 daemon 同样识别。关闭并保存会替换原自定义代理地址。

macOS 上通过 360 项核心测试、121 项前端测试、Rust 格式、UI 语法和独立 daemon 编译检查；核心网络测试在沙盒外监听本机测试端口，未使用真实账号。新增测试覆盖直连请求绕过代理、Provider 覆盖持久化、会话隔离及弹窗默认启用、关闭保存直连/重新启用恢复继承。已用真实 HTML/CSS 和示例账号渲染并检查弹窗预览；未进行 Tauri GUI 实机交互验证、Windows/Linux 实机验证或应用打包；现有预编译 daemon 未重新生成。

### Sub2api 刷新失败状态（2026-10-13）

Codex 和 Claude 的 Sub2api 刷新在配置读取、管理端登录或账号列表请求失败时，将错误保存到本次刷新涉及的缓存账号 `usageError`，保留已有额度；单卡失败不污染同 Provider 的其他卡片，整组失败标记该组账号。后续成功取得账号额度时清除错误。此前这些前置失败只返回命令错误，没有更新账号状态。单账号额度接口失败仍沿用原有返回账号错误的处理；本次没有统一订阅账号的令牌续期、限流退避或 Sub2api 的额度缓存策略。

macOS 上 362 项核心测试通过，新增回归覆盖单卡/整组失败、保留旧额度、错误隔离及成功恢复。测试使用临时目录、合成凭据和本机模拟服务，未操作真实账号；未进行 GUI 实机验证、Windows/Linux 验证或应用打包。

2026-10-13（代理图标状态）：账号卡片的地球图标默认隐藏，仅在指针悬停于该卡片或键盘焦点位于该卡片内时显示，显示时，显式直连或回退系统代理使用弱化颜色；独立代理、Sub2api Provider 代理或账号统一代理生效时保持正常颜色。隐藏时保留布局位置与键盘访问能力，避免布局跳动。121 项前端测试、JS 语法及 diff 检查通过；未进行 Tauri GUI 实机验证。

### 全局刷新遵循 Sub2api 单账号代理（2026-10-13）

Codex、Claude 的 Sub2api 全局刷新继续按 Provider 批量执行。同一 Provider 内按账号实际出口分组，相同代理或同为直连的账号共享一次管理端登录及列表查询；只有出口不同才分组请求，额度查询仍遵循各账号配置。修复原先批量前置请求忽略单账号直连/独立代理的问题。一个出口失败时，只更新该组缓存账号的错误，保留旧额度，其他组继续刷新；已导入但缓存缺失的账号仍可恢复。Provider 账号成员变化提示和显式同步导入新账号的行为保留。

macOS 上 364 项核心测试、Rust 格式及 diff 检查通过。回归覆盖同出口批量仅登录/查询列表一次、Provider/通用代理不可用时采用账号独立代理、显式直连、分组失败隔离及缺失缓存恢复。测试使用临时文件、合成凭据和本机模拟服务；未进行真实 Sub2api、Windows/Linux 或 GUI 实机验证，未重新打包安装。

### Claude 令牌续期限流退避（2026-10-13）

令牌续期接口返回 HTTP 429 时，与额度接口限流一样持久化 5 分钟冷却，覆盖提前续期和额度/资料请求失败后触发的续期路径。冷却期间单卡、全局及后台刷新不重复请求，重启后仍生效；到期后正常尝试，成功后清除错误和冷却。不将 429 误判为登录失效，不改变凭据同步及外部轮换优先规则。当前沿用固定 5 分钟策略，尚未按服务端 Retry-After 动态调整。

macOS 上 366 项核心测试、Rust 格式及 diff 检查通过，新增回归覆盖过期令牌/额度 401 触发续期、重启后冷却、手动及后台请求抑制、到期恢复，以及其他令牌错误不误分类。测试仅使用合成凭据、临时目录及本机模拟服务，未请求或更改真实令牌；未进行 Windows/Linux 验证或应用打包安装。

2026-10-13（General 控件对齐）：设置页 General 区块的代理输入框和下拉框文字右对齐；用量显示和语言下拉框按当前选项文本自适应宽度，在选项切换、语言切换和设置重新加载时同步。General 各行通过 CSS subgrid 共享标题列宽，代理输入框从该区块最长标题右侧 16px 处开始并保持右边缘对齐，随语言和窗口宽度自动调整；支持触控板横向滑动及 Shift+滚轮查看长地址，保留原生光标移动滚动行为。121 项前端测试、JS 语法及 diff 检查通过；未进行 Tauri GUI 或 Windows/Linux 实机验证，未重新打包。

2026-10-13（Claude 启动钥匙串授权）：修正此前仅手动导入/切换允许授权的限制。池内已有 Claude 官方账号时，启动后台任务先以 Interactive 模式读取本机登录，允许系统在需要时请求授权或解锁，成功后同步当前账号与凭据，再执行首次额度刷新；没有官方账号则不读取。每次启动只执行一次授权步骤，拒绝/读取失败保留账号池，后续列表读取、定时刷新和续期写回仍禁止弹窗；可通过手动导入或切换再次授权。Codex 首次刷新与该步骤并行。

验证：368 项核心测试、Rust 格式及 diff 检查通过；新增回归覆盖启动跟随外部当前账号、不主动刷新额度、空账号池不读取凭据、读取失败不修改池文件。测试使用临时文件及合成凭据，未访问真实钥匙串；真实系统授权窗口、Windows/Linux 未实机验证，未打包或替换正在运行的应用。

2026-10-13（卡片令牌过期时间）：账号卡片底栏在「额度 · 重置」后追加访问令牌剩余时间（按天/小时/分钟取最大整单位，以 Intl narrow 缩写本地化，如 6d/23h/59m），悬停显示具体过期时间及自动续期说明；已过期仅显示「已过期」。该段整体不断行：360px 窗口下英、中、日、韩、法、意、荷各组合均单行，德、西、俄在额度为「无限制」等较长组合时令牌段整体换至第二行。`AccountSummary` 新增 `tokenExpiresAt`（Unix 秒）：Codex 取 access_token JWT 的 `exp`，Claude 取 `expiresAt`；Sub2api 及无法解析的令牌不显示。refresh token 本身无可读过期时间，不展示。收起模式不显示。仅在显示文本变化时重绘卡片。

验证：368 项核心测试、122 项前端测试、独立 daemon 检查通过；新增回归覆盖 Codex/Claude 卡片携带过期时间、不透明令牌不显示及相对时间格式化；各语言底栏宽度在 Chrome 中以实际样式测量。测试仅使用合成令牌；未在 Tauri GUI（WebKit）中实机查看，未打包。

2026-10-14（令牌过期时间健壮性）：过期时间仅在 1970 年后至 9999-12-31 之间时显示（核心 `displayed_expiry` 与前端 `tokenExpiry` 同一范围），排除 0/负值、把毫秒当作秒等异常声明；此前超出 JS Date 范围的值会令悬停时间格式化抛错并中断整个账号列表渲染。Codex JWT `exp` 接受小数（RFC 7519 NumericDate），同时用于判断是否需要预先续期。Codex 已轮换 auth.json 中的登录而 Copool 尚未采纳时，当前账号卡片显示正在生效的那份令牌的过期时间。

验证：371 项核心测试、122 项前端测试、独立 daemon 检查通过；新增回归覆盖小数/0/负数/毫秒/字符串/缺失的 `exp`、前端非法值不渲染且不抛错，以及仅采纳更新的 auth.json 登录。未在 Tauri GUI 中实机查看，未打包。

2026-10-14（Claude Sub2api 自动刷新只读当前 Provider）：Claude 池的后台刷新不再刷新所有 Provider。此前启动的首个 tick 与每第 6 个 tick 是全池 tick，会经 `claude_refresh(None)` 刷新每个 Provider 的卡片，即使官方登录在用、没有任何 Provider 被选中，也会读取（并请求授权）Sub2api 管理员密码。现在自动路径与 Codex 池一致，只刷新“在用 Provider”——`settings_provider` 已设置且主设置非官方时 UI 上被标记为当前的卡片所属 Provider——并刷新其全部已导入卡片；当前为官方登录时，自动刷新不读任何 Provider 密码，也不联系任何 Provider。启动步骤仍只保留 Claude Code 凭据的一次互动授权（`claude_authorize_at_startup`，读取策略不变）。手动全量刷新（`claude_refresh(None)`）、单卡刷新与显式同步保持原语义，仍读取所有 Provider 的密码。

行为变化：非在用 Provider 的卡片保留上次快照，直到手动刷新或同步；在用 Provider 的每个 tick 都刷新其全部卡片（此前中间 tick 只刷新当前卡片）；不经 Copool 把 Claude Code 设置指向某个 Provider 时 `settings_provider` 为空，该 Provider 不自动刷新。

验证：`python3 scripts/test_rust.py` 通过（390 项核心测试、123 项前端测试、Rust 格式、UI 语法及独立 daemon 检查）。新增回归覆盖：官方登录在用时后台 tick 不读密码且不联系 Provider，随后手动刷新仍读取密码并命中管理端登录；切到 Provider 的卡片后中间 tick 只读该 Provider 的密码、刷新其全部卡片（两条出口各一次登录），另一 Provider 保持零请求；手动全量刷新读取并刷新两个 Provider。测试使用临时目录、合成凭据与本机模拟服务，未访问真实钥匙串；真实弹窗次数、GUI 与 Windows/Linux 未实机验证，未打包或安装。

2026-10-14（同一 Sub2api Provider 的卡片合并显示）：账号页把同一 Provider 的多张卡片收进一个分组，放在该 Provider 第一张卡片原本的位置（`providerGroups`）。只有一张卡片的 Provider 仍按普通卡片显示。组内账号按 Sub2api 账号 id（卡片 id 末尾的数字）升序排列，不按用量排名；分组本身的位置仍取其排名最靠前的卡片。卡片所属 Provider 从卡片 id 解析（`sub2api-<provider>-<n>`、`claude-sub2api-<provider>-<n>`），不依赖可能过期的 Provider 列表缓存；Claude 同步后也会重新读取 Provider 列表。分组标题只显示一次 Provider 名、SUB2API 来源标签和“当前”标签，按钮位置与单个账号卡片一致：代理和移除在右上角，刷新和切换在右下角。移除只能对整组进行，会移除该 Provider 的全部卡片（`removeAccounts`，结束后只重新加载一次）。组内各账号保持普通卡片的结构（标签行、名称、用量、额度和用量错误提示），只是没有自己的 SUB2API 标签、代理、移除、刷新、切换和“当前”；没有套餐标签时不显示空的标签行。折叠模式下没有标题行：同组账号显示为普通折叠行，状态点和刷新、切换按钮只在组内第一行出现，组内各行之间不画分隔线。刷新一次刷新该 Provider 的全部卡片：Codex 用现有的 `refresh_provider`（不同步成员），Claude 用新增的 `claude_refresh_provider`。分组代理会写入每张卡片；各卡片代理一致时预填该值，不一致时为空。分组切换经由组内任一可用卡片完成，核心层本来就按 Provider 切换。这是对原 SwiftUI 卡片布局的有意调整，由用户要求。

验证：`python3 scripts/test_rust.py` 通过（390 项核心测试、128 项前端测试）。新增回归覆盖：分组顺序、Provider 刷新只锁该组卡片且只调用一次、分组代理写入每张卡片。另在 headless Chrome 中用桩数据查看了展开与折叠两种布局。未在 Tauri GUI 中实机查看，未打包。

2026-10-14（拖动调整账号卡片顺序）：账号页的卡片可以拖动排序，交互参照 OpenQuota 的 `pointerReorder`，新代码在 `desktop/ui/reorder.js`。
- 展开模式下按住卡片的标签行或名称拖动；Sub2api 分组按住分组标题；折叠模式下按住整行。按钮不会触发拖动，移动不足 4px 也不算拖动。
- 拖动时原卡片留出空位，一张不透明的副本跟随指针（放大 1.018、带阴影）；指针进入相邻卡片超过 20% 时实时换位，其余卡片用 response 0.42s、阻尼 0.8 的弹簧动画让位。指针靠近滚动区域边缘会自动滚动。
- 与 OpenQuota 不同，不按“同一目标不连续触发”去重（那样换位后无法拖回原位）。改为按方向判断：换位后，指针需朝下一次换位的方向、在滚动内容中再移动至少 4px，才会再次换位。这样能拖回原位，高度不同的相邻卡片也不会来回抖动。
- 松开即保存；Esc、窗口失焦或 pointercancel 会恢复拖动前的顺序，且 Esc 不会同时关闭面板。拖动期间列表暂停重绘，松开后再按新顺序渲染。
- 顺序按池保存在偏好设置的 `accountOrder`（settings.json 的 `accountOrder`，形如 `{codex: [...], claude: [...]}`）。键是账号 id；Sub2api Provider 的卡片无论单张还是成组，都共用 `sub2api:<provider>` 一个键。
- 激活的卡片（或包含激活账号的 Sub2api 分组）始终排在首位，不受保存顺序影响；它不能被拖动，其他卡片也不能拖到它上面（`data-reorder-fixed`）。保存的顺序包含它当时的位置，它不再激活后回到保存的位置。激活变化时（例如切换账号），新激活的卡片用与拖动相同的弹簧动画从原位置升到首位，途中显示在其他卡片之上，被越过的卡片同时让位；激活不变的重绘没有动画，切换池或展开/折叠时也不播放。
- 其余卡片中，已保存的键按保存顺序排在前面；未保存过的卡片（例如新导入的）按原有排名接在后面；已不存在的键直接忽略。保存的值无法解析时视为没有自定义顺序。
- 这只影响账号页的显示顺序，智能切换、自动切换和小组件仍按原排名。组内账号仍按 Sub2api 账号 id 排列，不可拖动。暂未提供 OpenQuota 的 Alt+方向键键盘排序。

验证：`python3 scripts/test_rust.py` 通过（391 项核心测试、134 项前端测试）。新增回归覆盖：偏好设置中顺序的往返与损坏值容错、保存顺序与新卡片的合并、换位阈值、边缘滚动速度与弹簧曲线、拖过一张或两张卡后拖回原位、高个相邻卡片换位后不立即换回、激活卡片始终在首位。另在 headless Chrome 中用 DevTools 协议发送真实鼠标事件，在桩数据页面上验证了：展开模式拖动后实时换位并保存一次；移动 2px 不触发拖动；Esc 恢复原顺序、不保存、不关闭面板；折叠模式拖动生效；拖下去再拖回来回到原位且不保存；保存顺序把激活账号排在最后时它仍在首位，拖它不会浮起，把其他卡片拖到最上方也停在它之后且不保存；切换到列表底部的账号后，它在约 630ms 内升到首位，期间位于其他卡片之上，普通重绘没有动画。未在 Tauri GUI 中实机拖动，未打包。
