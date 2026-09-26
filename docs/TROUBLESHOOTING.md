# Troubleshooting

## `CONFIG_INVALID`

只支持 loopback HTTP 代理。检查 `[proxy]` 的 `scheme`、`host` 和 `port`。SOCKS-only
端口不能直接填写；请使用代理软件提供的 HTTP/Mixed 端口。

无需手动编辑文件：在 TUI 主界面按 `C`，用 `Ctrl-U` 清空端口后输入新值，再按 `Enter`
保存。

## 双击打开后立即退出

无子命令启动时，旧版本或无效配置不会再使应用直接退出，而是进入配置修复界面。按 `C`，
填写本机 HTTP/Mixed 的 host 和 port 后按 `Enter` 覆盖旧配置。若终端仍然立刻关闭，请从
PowerShell 运行程序以查看 Windows 或终端初始化错误。

## `CODEX_NOT_INSTALLED`

请安装当前 ChatGPT Desktop，而不是只依赖可能同时存在的 ChatGPT Classic。官方下载页为
<https://chatgpt.com/download/>，Microsoft Store 产品 ID 为 `9PLM9XGG6VKS`：

```powershell
winget install --id 9PLM9XGG6VKS -s msstore
```

Guard 在当前 ChatGPT Desktop 不存在时才回退到 Classic；两者都存在时会选当前应用。若组织
使用自定义部署路径，可配置绝对路径：

```toml
[codex]
executable_override = "D:\\Path\\To\\ChatGPT.exe"
```

## `CODEX_ALREADY_RUNNING`

现有 ChatGPT Desktop 进程无法事后继承新环境。请从系统托盘完全退出 ChatGPT，然后在
Guard 中按 `R` 刷新并重新启动。Guard 不提供强制终止。

## `LAUNCH_BUSY`

另一 Guard 实例正在执行启动。等待其完成后重试；这是防止并发启动两个 Desktop 的
安全锁。

## `ELEVATED_LAUNCH_UNSUPPORTED`

Guard 自身正在以管理员身份运行。Codex 0.157+ 的共享后台服务必须由非提升进程启动，
因此 Guard 在提升运行时会直接阻止 Launch。

请关闭当前的管理员实例，正常双击启动 Codex Proxy Guard。不要尝试用 UAC、降权或代理
进程绕过；Guard 与 Codex 均不支持该用法。

## `CODEX_DAEMON_STOP_FAILED`

Guard 找到了官方 Codex CLI，但 `codex app-server daemon stop` 明确失败。此时可能存在
持有旧代理环境的后台 daemon，Guard 不会在此状态下启动 Desktop。

建议手动执行：

```powershell
codex app-server daemon stop
```

然后重新启动 Guard 并 Launch。若 CLI 反复失败，请确认安装的是官方 Codex CLI；必要时
可用 `[codex] cli_executable_override` 指定其 `codex.exe` 的绝对路径。

## `CODEX_DAEMON_STOP_TIMEOUT`

Codex daemon 默认有 60 秒的 graceful shutdown 窗口，Guard 等待 75 秒后超时并只终止 CLI
子进程（绝不直接杀 daemon）。本次 Launch 已被阻止。

可手动执行并等待其完成：

```powershell
codex app-server daemon stop
```

确认关闭后重新启动 Guard 并 Launch。

## 切换代理端口后 Codex 仍走旧代理

Codex 0.157+ 的常驻 daemon 保留其启动时继承的环境变量，旧 daemon 存在时新 Desktop 的
代理环境不会生效。Guard 会在启动前通过官方 `daemon stop` 刷新它：成功启动后如果流量
仍走旧端口，先完全退出 Desktop，再手动执行 `codex app-server daemon stop`，然后通过
Guard 重新启动。

若 Launch 回执显示 `daemon_preparation` 为 `lifecycle_unavailable`，说明本机未找到官方
Codex CLI（或版本早于 0.157）。此时 Guard 仍按传统环境注入启动；如受 stale daemon 影响，
请安装或更新官方 Codex CLI 后重试。

## Desktop 启动后无法联网

Guard 不检测代理可用性。请在代理软件中确认：

1. HTTP/Mixed 端口与配置一致；
2. 代理软件正在运行；
3. 当前节点和路由可用。

若能登录但对话卡住或流式输出中断，还应确认代理、防火墙或安全网关允许 HTTPS 与 WebSocket
Upgrade，且不会改写 TLS 或过早关闭长连接。ChatGPT 使用 `wss://ws.chatgpt.com`，Codex
使用 `wss://chatgpt.com/`。Guard 不会为这些情况发起探测。

ChatGPT Voice 可能优先使用 UDP，因此 HTTP/Mixed 代理环境不保证覆盖它。然后完全退出
ChatGPT Desktop，再通过 Guard 重新启动。

## 配置来自旧版本或代理端口变更

旧版本配置不会迁移。执行 `codex-proxy-guard init-config --force` 生成当前最小配置；该命令会覆盖现有文件，应先记录需要保留的代理端点。

代理软件不限于 v2rayN，但必须提供本机 HTTP/Mixed 端口。若实际端口为 `7890`，可执行：

```powershell
codex-proxy-guard init-config --force --proxy-host 127.0.0.1 --proxy-port 7890
```

或手动更新 `[proxy]` 的 `host` 与 `port`。SOCKS-only 端口不能直接使用。

## Guard 退出后 Desktop 仍在运行

这是预期行为。Guard 只负责启动时注入环境，不托管 Desktop 生命周期。
