# Codex Proxy Guard

Codex Proxy Guard 是一个单一职责的 Windows 启动器：使用用户配置的本机 HTTP/Mixed 代理
启动 ChatGPT Desktop（Chat、Work 和 Codex），并在必要时通过 Codex 官方生命周期命令刷新
旧后台 daemon，确保新的代理环境真正生效。

它不修改 Windows 系统代理，不读取认证信息，不检测代理质量，也不管理 v2rayN。

## 功能边界

保留的能力：

- 只接受 `localhost`、`127.0.0.0/8` 或 `::1` 上的 HTTP 代理；
- 自动发现当前 ChatGPT Desktop，并在其不存在时回退到 ChatGPT Classic；
- 支持显式可执行文件 override；
- 启动前确认 Desktop 根进程没有运行；
- 拒绝在管理员权限的 Guard 中启动（Codex 0.157+ 后台服务要求非提升进程）；
- 启动前通过官方 `codex app-server daemon stop` 清理持有旧环境的后台 daemon；
- 注入大小写两套 `HTTP_PROXY`、`HTTPS_PROXY` 和 `NO_PROXY`；
- 清除新进程树中的 `ALL_PROXY` / `all_proxy`；
- 使用跨 Guard 实例的启动锁，避免并发启动竞态；
- Guard 退出不终止 Desktop。

明确不包含：

- TCP、HTTP CONNECT、TLS/HTTPS 或 WebSocket 探测；
- Node Readiness、Usage 查询、历史与导出；
- `codex doctor` 或日志扫描；
- v2rayN 发现、启动、切换节点或进程管理；
- 启动、重启、更新或监控 Codex daemon，连接 app-server 私有 IPC，或直接终止 Codex 进程；
- 强制终止 Desktop；
- 系统代理、TUN、WFP、WinDivert、Hook、Relay 或 TLS 解密。

Guard 只设置新进程树的代理环境，不是流量强制隧道：它不会接管 DNS、UDP 或任何其他
非 HTTP 代理路径，也不会判断应用是否真的通过代理联网。

## 快速开始

要求 Windows 10/11 与 Rust 1.85 或更新版本。

```powershell
cargo build --release -p codex-proxy-guard
./target/release/codex-proxy-guard.exe init-config
./target/release/codex-proxy-guard.exe
```

无子命令时打开单屏 TUI：

| 按键 | 行为 |
| --- | --- |
| `Enter` / `L` | 通过配置的代理启动 Desktop |
| `R` | 刷新 Desktop 发现与运行状态 |
| `?` | 查看帮助 |
| `Q` / `Ctrl-C` | 退出 Guard，不终止 Desktop |

脚本化启动：

```powershell
codex-proxy-guard launch
codex-proxy-guard launch --json
codex-proxy-guard config-path
```

`launch --json` 的回执除了 PID 与代理端点外，还会包含所选应用的产品类型、包名、版本、
架构、发现来源，以及本次启动的 daemon 准备结果（`not_needed` / `stopped` /
`lifecycle_unavailable`）；不会输出本地安装路径或认证信息。

## Codex 0.157+ daemon 兼容

Codex 0.157 起默认启用常驻后台 daemon，daemon 保留其启动时继承的环境变量；新打开的
Desktop 进程无法改变已在运行的 daemon 的环境。因此在启动 Desktop 前，Guard 会先尝试：

```text
codex app-server daemon stop
```

由 Codex 官方 CLI 正常关闭旧 daemon；随后 Desktop 以注入的代理环境启动，并由 Codex
自行创建新的 daemon。Guard 不启动、不重启、不监控 daemon，不连接其私有 IPC，也绝不
直接终止 Codex 进程。

CLI 解析顺序：`codex.cli_executable_override` → `%CODEX_HOME%\packages\app-server-daemon\current\bin\codex.exe`
→ `%CODEX_HOME%\packages\standalone\current\bin\codex.exe`（默认 CODEX_HOME 为
`%USERPROFILE%\.codex`）→ `where.exe codex.exe`。找不到 CLI 或旧版 Codex 不支持该命令时，
Guard 不会阻止启动，仍按传统环境注入流程进行。

另外，Codex 0.157+ 拒绝从提升（管理员）进程启动共享后台服务，因此 Guard 自身以管理员
身份运行时会直接阻止 Launch（`ELEVATED_LAUNCH_UNSUPPORTED`）。请正常双击启动 Guard，
不要“以管理员身份运行”。

## 支持的 ChatGPT Desktop 与安装

Guard 首选当前 ChatGPT Desktop（现有 Codex 用户更新后得到的统一应用），并在它未安装时
使用 ChatGPT Classic。两者同时存在时始终选择当前应用，不比较两个产品各自的版本号。

安装当前 ChatGPT Desktop：<https://chatgpt.com/download/>。也可使用 Microsoft Store 产品 ID
`9PLM9XGG6VKS`：

```powershell
winget install --id 9PLM9XGG6VKS -s msstore
```

自动发现读取每个已知 APPX 包的清单入口，并要求入口解析后仍位于该包的安装目录内；仅在
清单入口缺失或不存在时才使用受控的 `app\ChatGPT.exe` / `app\Codex.exe` 后备路径。

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

[tui]
alternate_screen = "auto"
```

`10808` 只是首次生成配置的默认示例端口；请替换为实际代理软件的 HTTP/Mixed 端口。
`executable_override` 指 ChatGPT Desktop 可执行文件；`cli_executable_override` 指官方
Codex CLI 可执行文件（仅用于 daemon 生命周期命令，留空时自动解析）。`cli_executable_override`
是 V2 schema 的可选扩展，已有的 V2 配置无需修改即可继续使用。

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
