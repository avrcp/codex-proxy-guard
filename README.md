# Codex Proxy Guard

Codex Proxy Guard 是一个单一职责的 Windows 启动器：为新启动的 ChatGPT Desktop
（Chat、Work 和 Codex）进程树注入 loopback HTTP 代理环境。

它不修改 Windows 系统代理，不读取认证信息，也不接管 Desktop 生命周期。

V2 提供两种模式：

- **External Mode**（默认）：使用你已有的本机 HTTP/Mixed 代理端口，行为与 V1 一致。
- **Managed Mode**：直接加载机场订阅，只保留 JP/SG/US 节点，用受控 benchmark 选出
  JP 硬优先的节点，并由 Guard 自己启动 sing-box sidecar。Desktop 仍然只收到
  `HTTP_PROXY`/`HTTPS_PROXY`。

## 功能边界

保留的能力：

- 只接受 `localhost`、`127.0.0.0/8` 或 `::1` 上的 HTTP 代理；
- 自动发现当前 ChatGPT Desktop，并在其不存在时回退到 ChatGPT Classic；
- 支持显式可执行文件 override；
- 启动前确认 Desktop 根进程没有运行；
- 注入大小写两套 `HTTP_PROXY`、`HTTPS_PROXY` 和 `NO_PROXY`；
- 清除新进程树中的 `ALL_PROXY` / `all_proxy`；
- 使用跨 Guard 实例的启动锁，避免并发启动竞态；
- Guard 退出不终止 Desktop。

明确不包含：

- TCP、HTTP CONNECT、TLS/HTTPS 或 WebSocket 探测（External Mode）；
- Node Readiness、Usage 查询、历史与导出；
- `codex doctor` 或日志扫描；
- v2rayN 发现、启动、切换节点或进程管理；
- 强制终止 Desktop；
- 系统代理、TUN、WFP、WinDivert、Hook、Relay 或 TLS 解密；
- 读取 Token/Cookie/OAuth/认证文件或调用 Codex 私有 API。

Managed Mode 的 benchmark 只对 `https://chatgpt.com/` 做受控、低频、限时的 HEAD 与 Geo
查询，用于 JP/SG/US 之间选节点，不做测速带宽、账号资格或真实 prompt 测试。

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
| `S` | 同步已保存的订阅（Managed Mode） |
| `B` | Benchmark JP/SG/US 节点（Managed Mode） |
| `?` | 查看帮助 |
| `Q` / `Ctrl-C` | 退出 Guard，不终止 Desktop |

脚本化启动：

```powershell
codex-proxy-guard launch
codex-proxy-guard launch --json
codex-proxy-guard config-path
```

`launch --json` 的回执除了 PID 与代理端点外，还会包含所选应用的产品类型、包名、版本、
架构和发现来源；不会输出本地安装路径或认证信息。

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
version = 3

[proxy]
mode = "external"
scheme = "http"
host = "127.0.0.1"
port = 10808
no_proxy = ["localhost", "127.0.0.1", "::1"]

[managed]
subscription_id = ""
sing_box_path = ""
benchmark_cache_hours = 6

[codex]
executable_override = ""
refuse_if_running = true

[tui]
alternate_screen = "auto"
```

`10808` 只是首次生成配置的默认示例端口；请替换为实际代理软件的 HTTP/Mixed 端口。

这是唯一支持的配置结构：旧版配置不会迁移或忽略字段，而是会被拒绝。需要重置时执行
`codex-proxy-guard init-config --force`。

`proxy.mode` 为 `"managed"` 时启用 Managed Mode，忽略手写的 `proxy.port`，改用 Guard
启动的 ephemeral loopback 端点；此时 `managed.subscription_id` 必须指向一个已添加的
订阅。`managed.sing_box_path` 留空时自动发现
`%APPDATA%\codex-proxy-guard\runtime\sing-box\current\sing-box.exe`。

代理软件不限于 v2rayN。只要它提供本机 HTTP/Mixed 入站端口，就将实际的 host 和 port
写入 `[proxy]`；SOCKS-only 端口不适用。例如 Clash、sing-box 或其他代理监听在
`127.0.0.1:7890` 时：

```powershell
codex-proxy-guard init-config --force --proxy-host 127.0.0.1 --proxy-port 7890
```

也可以直接修改现有配置中的 `host` 和 `port`。Guard 不探测或管理代理软件；它只校验端点
是 loopback HTTP/Mixed 代理，并将该端点注入新启动的 Desktop 进程。

## Managed Mode

订阅 URL 很长且含 token，通过 CLI 配置，只保存在 Windows Credential Manager 的
`CodexProxyGuard.Subscription` 命名空间下，绝不写 TOML、日志或 JSON 输出：

```powershell
codex-proxy-guard subscription add --name "Airport" --url "https://..."
codex-proxy-guard subscription list
codex-proxy-guard subscription sync "Airport"
codex-proxy-guard node list                 # 或 --region JP|SG|US
codex-proxy-guard benchmark                 # 或 --force / --json
codex-proxy-guard best-node
codex-proxy-guard subscription delete "Airport" --yes
```

同步只导入名称可归类为 JP/SG/US 的节点（其余计入 IgnoredRegion），随后 benchmark 校验
真实出口国：国家与 hint 不一致的节点永远不能成为 winner。选择是字典序 `JP > SG > US`，
即只要有 Healthy JP 就一定选 JP。

把 `proxy.mode = "managed"` 写入配置后，直接运行 `codex-proxy-guard` 即可看到 Managed
TUI（`S` 同步、`B` benchmark、`Enter` 启动）。若无 Healthy 节点则不启动 Desktop。

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
