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
