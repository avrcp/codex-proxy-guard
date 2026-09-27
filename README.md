# Codex Proxy Guard

Codex Proxy Guard 是一个单一职责的 Windows 启动器：使用用户配置的本机 HTTP/Mixed 代理
启动 ChatGPT Desktop（Chat、Work 和 Codex）。已注册的 Desktop 应用通过 Windows
注册入口原生激活（`IApplicationActivationManager::ActivateApplication`，`AO_NONE`），
普通可执行文件则注入进程环境；普通启动对 Codex 共享后台服务零副作用，仅在用户单次
显式授权的修复启动中通过 Codex 官方生命周期命令停止旧共享服务。

它不修改 Windows 系统代理，不读取认证信息，不检测代理质量，也不管理 v2rayN。

## 功能边界

保留的能力：

- 只接受 `localhost`、`127.0.0.0/8` 或 `::1` 上的 HTTP 代理；
- 自动发现当前 ChatGPT Desktop，并在其不存在时回退到 ChatGPT Classic；
- 支持显式可执行文件 override；包内 override 仍按已注册包处理；
- 启动前确认 Desktop 根进程没有运行；
- 拒绝在管理员权限的 Guard（或权限查询失败）中启动；
- 普通启动不解析 Codex CLI、不执行任何 daemon 命令；
- 显式修复启动（单次确认）通过官方 `codex app-server daemon stop` 停止共享服务；
- 未打包 EXE：注入大小写两套 `HTTP_PROXY`、`HTTPS_PROXY` 和 `NO_PROXY`，并清除
  `ALL_PROXY` / `all_proxy`；
- 已注册应用：经校验的 Chromium `--proxy-server` / `--proxy-bypass-list` 激活参数
  （`no_proxy` 中的 loopback 项逐项映射，无法等价转换的条目报错而不是丢弃）；
- 可选（默认关闭、需单次显式授权）：在已确认 Codex Home 的 `.env` 中管理一个
  BEGIN/END 标记的 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` 块，供 Codex 后端读取；
- 使用跨 Guard 实例的启动锁，避免并发启动竞态；
- Guard 退出不终止 Desktop；
- 激活后核验实际目标进程的精确程序包身份、应用身份（AUMID）、创建时间与提升状态。

明确不包含：

- TCP、HTTP CONNECT、TLS/HTTPS 或 WebSocket 探测；
- Node Readiness、Usage 查询、历史与导出；
- `codex doctor` 或日志扫描；
- v2rayN 发现、启动、切换节点或进程管理；
- 启动、重启、更新或监控 Codex daemon，连接 app-server 私有 IPC，或直接终止 Codex 进程
  （Guard 仅终止自己创建的短命 worker / CLI 子进程）；
- 强制终止 Desktop；
- 系统代理、TUN、WFP、WinDivert、Hook、Relay 或 TLS 解密；
- 裸启动 WindowsApps 内 EXE、`Invoke-CommandInDesktopPackage` 调试上下文、包调试模式
  或任何提权/降权技巧作为注册应用的启动方式。

Guard 只交付代理启动计划，不是流量强制隧道：它不会接管 DNS、UDP 或任何其他非 HTTP
代理路径，也不会判断应用是否真的通过代理联网。激活参数覆盖 Electron/Chromium 的
HTTP 路径；授权的 Home 块覆盖后续从该 Home 启动的 Codex 后端——两者是分别陈述的事实。

## 快速开始

要求 Windows 10/11 与 Rust 1.88 或更新版本。

```powershell
cargo build --release -p codex-proxy-guard
./target/release/codex-proxy-guard.exe init-config
./target/release/codex-proxy-guard.exe
```

无子命令时打开单屏 TUI：

| 按键 | 行为 |
| --- | --- |
| `Enter` / `L` | 普通启动：已注册应用走 Windows 注册入口原生激活，普通 EXE 注入进程环境 |
| `D` | 修复启动：确认后先停止共享 Codex 后台服务再启动（`Y` 确认 / `N`、`Esc` 取消；已注册目标需先 `B` 授权） |
| `B` | 授权或撤销 Codex Home `.env` 后端代理块（显示确切 Home 与作用范围，`Y` 确认） |
| `R` | 刷新 Desktop 发现与运行状态 |
| `C` | 编辑代理地址和端口 |
| `?` | 查看帮助（含构建版本与提交） |
| `Q` / `Ctrl-C` | 退出 Guard，不终止 Desktop |

脚本化启动：

```powershell
codex-proxy-guard launch
codex-proxy-guard launch --json
codex-proxy-guard launch --activation-only
codex-proxy-guard launch --refresh-codex-daemon
codex-proxy-guard build-info
codex-proxy-guard config-path
```

`launch --json` 的回执将事实分层陈述：启动方法（`native_process` /
`appmodel_activation`）、激活状态、实例观察（created / reused / unknown）、包身份与应用
身份（AUMID）观测、代理交付层（进程环境 / 激活参数 / 激活参数+Home 配置 /
未交付）、后端配置状态、目标提升状态，以及 daemon 准备结果（`skipped` / `stopped` /
`not_needed`）。不会输出本地安装路径或认证信息。回执只陈述 Guard 观测到的事实；
不表示已验证界面、联网、沙箱或业务流量确实走了代理。

`launch --activation-only` 是诊断入口：只做原生激活、不带代理参数、不触碰 `.env`，
回执明确 `proxy_delivery = not_established`；它不是普通启动的隐藏降级。

## 普通启动、原生激活与显式修复启动

**普通启动（默认，`Enter` / `launch`）**：配置校验 → 权限检查 → 发现 Desktop →
启动锁 → 确认未运行。已注册应用使用动态解析的 AUMID（`PackageFamilyName!ApplicationId`）
调用 Windows 应用激活（`AO_NONE`，正常注册入口激活），激活参数携带经校验的 Chromium
代理项；激活返回后用持有的进程句柄核验目标 PackageFullName、AUMID、映像路径、创建时间
与提升状态。普通未打包 override 在创建处注入代理环境。普通路径不解析 Codex CLI、
不执行 daemon 命令。

COM 调用运行在 Guard 自己的短命 worker 进程中（同一 EXE 的隐藏
`internal-activate-package` 子命令，16 KiB 请求 / 64 KiB 回执的有界 stdin/stdout 协议），
worker 本身不需要 OpenAI 包身份，也不运行在包上下文中。提交前取消保证不会发起激活；
提交后取消或超时按 `APPX_ACTIVATION_OUTCOME_UNKNOWN` 如实报告（Windows 可能已经完成
激活），不会自动重试，也不会终止 Desktop。

**后端代理配置（`B`，默认关闭）**：激活接口没有环境块参数，Chromium 参数也覆盖不了
Codex 后端进程。若你显式授权，Guard 会在**一个已授权绑定的 Codex Home** 的 `.env` 中维护
自己的 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` 标记块（原子写入、逐字节保留其他内容、
冲突键——含 `export KEY=...` dotenv 写法——拒绝覆盖）。撤销先安全移除 Guard 自己的块、
成功后才关闭授权：revoke 失败时授权、绑定 Home 与文件原样保留，可直接重试，不会留下
Guard 无法再定位的孤儿块。该块会影响之后从同一 Home 启动的所有 Codex 客户端，
不只本次 Desktop；授权与作用范围见 TUI 提示。Guard 绝不从自己的 `CODEX_HOME` 推断
Desktop 的 Home；未授权时回执明确 `backend_proxy_config = not_authorized`，不会静默
降级为"可能直连的代理启动"。

**显式修复启动（`D` 键确认后 `Y`，或 `launch --refresh-codex-daemon`）**：Codex 0.157+
的常驻共享 daemon 保留其启动时继承的环境变量，已运行的旧 daemon 不会因新 Desktop
启动而更新代理环境。若你需要丢弃旧 daemon 环境，可以显式授权一次修复。对已注册的
Desktop，修复启动需要先经 `B` 授权后端代理块（未授权时 `D` 会被
`BACKEND_PROXY_REQUIRED_FOR_REPAIR` 阻止）；Guard 先把授权 Home 的 `.env` 受管块准备
就绪，确认无误后才执行官方 `codex app-server daemon stop`，再激活 Desktop，由 Codex
自行创建新的 daemon——prepare 失败时共享服务必然未被中断。未打包目标不受此门禁
限制（进程环境注入本身就携带新代理）。

`daemon stop` 会中断同一 Codex Home 下其他 CLI / IDE / 远程客户端正在执行的任务，因此
它绝不在普通启动中隐式执行：CLI 入口每次调用都要带 `--refresh-codex-daemon`，TUI 中
`D` 之后必须再按 `Y` 单次确认（默认取消）。修复路径中 stop 失败、超时（总预算 720 秒，
期间可随时取消）或输出无法识别都会阻止本次启动，不会悄悄降级为普通启动。

程序包身份弹窗本身不需要停止 daemon；原生激活失败也不会自动 stop。不要把 `D` 当成
身份修复步骤。

Guard 不启动、不重启、不更新、不监控 daemon，不连接其私有 IPC。唯一例外是 Guard 自己
创建的短命 worker / CLI 子进程：超时或取消时会被终止并回收。注意：stop 成功也不保证
新 daemon 已继承代理——若需要后端也走代理，请使用上面 `B` 授权的 `.env` 代理块（这是
Codex 官方 CLI 文档化的 `.env` 读取行为；Guard 只管理自己的标记块，不读取也不改动
其中其他内容）。

Codex CLI 解析顺序（仅显式修复时）：`codex.cli_executable_override`（必须是现存绝对
路径的 native `.exe`，失效即报错不换源）→ `%CODEX_HOME%\packages\app-server-daemon\current\bin\codex.exe`
→ `%CODEX_HOME%\packages\standalone\current\bin\codex.exe`（含 legacy 平铺布局；
显式 CODEX_HOME 必须存在，不会回退默认目录）→ PATH 中的绝对目录（跳过当前目录、空项
与相对项，不使用 `where.exe`）。

另外，Codex 0.157+ 的共享后台服务拒绝从提升（管理员）进程启动，因此 Guard 自身以管理员
身份运行时会直接阻止 Launch（`ELEVATED_LAUNCH_UNSUPPORTED`；权限查询失败同样阻止，
`ELEVATION_QUERY_FAILED`）。请正常双击启动 Guard，不要“以管理员身份运行”。

注意区分版本：Codex CLI（如 0.157.1）与 ChatGPT Desktop 是两个独立产品，CLI 版本不能
当作 Desktop 的包版本；`launch --json` 回执中的 `package_version` 才是 Desktop 的版本。

## 支持的 ChatGPT Desktop 与安装

Guard 首选当前 ChatGPT Desktop（现有 Codex 用户更新后得到的统一应用），并在它未安装时
使用 ChatGPT Classic。两者同时存在时始终选择当前应用，不比较两个产品各自的版本号。

安装当前 ChatGPT Desktop：<https://chatgpt.com/download/>。也可使用 Microsoft Store 产品 ID
`9PLM9XGG6VKS`：

```powershell
winget install --id 9PLM9XGG6VKS -s msstore
```

自动发现读取已注册包的 FullName、FamilyName 与 Application.Id，并选择可证明的桌面
主入口；多个无法区分的入口会报错。入口解析后必须仍位于包安装目录内，缺失的清单
入口不会退化为包内 EXE 路径猜测。发现脚本源码是 `resources/appx-discovery.ps1`
（固定 `schema_version=1` envelope、显式序列化深度、可选清单属性允许为 null），
生产与测试执行同一份文件，可用 `powershell -File` 手工核对。

## 配置

默认路径为 `%APPDATA%\codex-proxy-guard\config.toml`：

```toml
version = 2

[proxy]
scheme = "http"
host = "127.0.0.1"
port = 10808
no_proxy = ["localhost", "127.0.0.1", "::1"]

[codex]
executable_override = ""
cli_executable_override = ""
refuse_if_running = true
manage_codex_proxy_env = false
proxy_env_home = ""

[tui]
alternate_screen = "auto"
```

`10808` 只是首次生成配置的默认示例端口；请替换为实际代理软件的 HTTP/Mixed 端口。
`executable_override` 指 ChatGPT Desktop 可执行文件；`cli_executable_override` 指官方
Codex CLI 可执行文件（仅用于 daemon 生命周期命令，留空时自动解析）。
`manage_codex_proxy_env` 与 `proxy_env_home` 是 V2 schema 的可选扩展（默认关闭）：
`.env` 代理块只在通过 TUI `B` 单次确认后才会启用，且绑定确认时显示的确切 Home；
已有 V2 配置无需修改即可继续使用。

这是唯一支持的配置结构：旧版配置不会迁移或忽略字段，而是会被拒绝。需要重置时执行
`codex-proxy-guard init-config --force`。

代理软件不限于 v2rayN。只要它提供本机 HTTP/Mixed 入站端口，就将实际的 host 和 port
写入 `[proxy]`；SOCKS-only 端口不适用。例如 Clash、sing-box 或其他代理监听在
`127.0.0.1:7890` 时：

```powershell
codex-proxy-guard init-config --force --proxy-host 127.0.0.1 --proxy-port 7890
```

也可以直接修改现有配置中的 `host` 和 `port`。Guard 不探测或管理代理软件；它只校验端点
是 loopback HTTP/Mixed 代理，并将该端点注入新启动的 Desktop 进程。

## 网络边界与手工验证

代理需要能正常转发 HTTPS 和 WebSocket Upgrade。ChatGPT 的对话更新使用
`wss://ws.chatgpt.com`，Codex 的流式交互使用 `wss://chatgpt.com/`；若代理或网关拦截、
改写 TLS，或过早关闭长连接，应用可能在启动后报网络错误。ChatGPT Voice 优先使用 UDP，
不应被视为受 HTTP 代理环境保证的流量。

发布前请手工检查登录、Chat 流式输出、Work、Codex、文件上传和内置浏览器。Guard 本身不会
为这些功能增加运行时探测、遥测或历史记录。

也可在 TUI 中按 `C` 直接编辑代理地址和端口（默认选中端口）：`Ctrl-U` 清空当前字段，
`Tab` 或方向键切换字段，`Enter` 保存，`Esc` 放弃更改。

若已有配置已过期或无效，直接打开程序会进入配置修复界面而不会立即退出；按 `C` 保存新的
有效端点即可覆盖旧文件。

## 验证与构建

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo audit
./scripts/build-portable.cmd
```

Portable 产物固定包含 EXE、SHA-256 和 `build-info.json`。

## Windows Release

Windows portable 版本发布在 [GitHub Releases](https://github.com/avrcp/codex-proxy-guard/releases)。
下载后可直接运行 `codex-proxy-guard-windows-x86_64.exe`；同目录的 `.sha256` 用于校验文件完整性，
`build-info.json` 记录构建版本、目标平台、提交和 Authenticode 状态。

更多信息见 [架构](docs/ARCHITECTURE.md)、[安全边界](docs/SECURITY.md)、
[故障排查](docs/TROUBLESHOOTING.md) 和 [发布清单](docs/RELEASE_CHECKLIST.md)。
