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

## `ELEVATION_QUERY_FAILED`

Guard 无法查询自身的权限状态（Win32 token 查询失败）。查询失败不会被视为“非提升”，
而是直接阻止 Launch，避免在权限未知时启动。

通常重开 Guard 即可恢复；若持续出现，请检查运行环境中是否有安全软件干扰进程 token
查询。

## 普通启动与修复启动的区别

普通启动（`Enter` / `launch`）不解析 Codex CLI、不执行任何 daemon 命令：它对共享
后台服务零副作用，也不会因为本机没有 Codex CLI 而失败。只有你显式授权的修复启动
（TUI 按 `D` 再按 `Y`，或 `launch --refresh-codex-daemon`）才会执行
`codex app-server daemon stop`——该命令可能中断同一 Codex Home 下其他 CLI / IDE /
远程客户端正在执行的任务，因此必须逐次确认，授权不会保存。

## `CODEX_CLI_UNAVAILABLE` / `CODEX_CLI_OVERRIDE_INVALID` / `CODEX_HOME_INVALID`

显式修复启动找不到可用的官方 Codex CLI，或配置的 override / `CODEX_HOME` 无效。这些
错误不会静默降级为普通启动。

- 未安装 CLI：可先普通启动；如确需刷新共享服务，请安装官方 Codex CLI 后重试；
- override 无效：`codex.cli_executable_override` 必须是现存绝对路径的 native
    `codex.exe`（不支持 `.cmd`/`.ps1` shim）；
- 显式 `CODEX_HOME` 必须是现存目录；未设置时使用用户目录下的默认 `.codex`。

## `CODEX_DAEMON_UNSUPPORTED`

已解析的 Codex CLI 不支持 `app-server daemon` 生命周期命令（常见于较旧版本）。修复
启动被阻止，但普通启动不受影响——直接按 `Enter` 即可按传统环境注入启动。

## `CODEX_DAEMON_STOP_FAILED`

显式修复中 `codex app-server daemon stop` 明确失败（非零退出、输出超限、非法 JSON、
未知状态等）。Guard 不会把失败解释成“服务不存在”，也不会在此状态下启动 Desktop。

可在确认没有需要保留的共享任务后手动执行：

```powershell
codex app-server daemon stop
```

然后重新通过 Guard 启动。若反复失败，可用 `[codex] cli_executable_override` 指定官方
`codex.exe` 的绝对路径。

## `CODEX_DAEMON_STOP_TIMEOUT`

修复步骤未在总预算（720 秒）内完成。共享服务可能已收到停止请求，最终状态未确认；
Guard 只终止了自己创建的 CLI 子进程，绝不直接杀 daemon，Desktop 未启动。

可手动执行并等待其完成，确认关闭后再重试：

```powershell
codex app-server daemon stop
```

等待期间也可以直接取消（TUI 的退出、CLI 的 Ctrl-C）；取消同样意味着共享服务状态
未确认，不要假设它没有变化。

## 切换代理端口后 Codex 仍走旧代理

Codex 0.157+ 的常驻共享 daemon 保留其启动时继承的环境变量；旧 daemon 存在时，新
Desktop 注入的代理环境不会影响它。需要丢弃旧环境时，使用显式修复启动（`D`+`Y` 或
`launch --refresh-codex-daemon`）。

两点提醒：

1. Guard 无法确认你的 Desktop 一定连接该共享服务（Codex CLI 与 Desktop 是独立产品，
   也可能存在不同 Codex Home 或非共享后端）。修复启动针对的是“已解析 Codex Home 中的
   共享服务”；若修复后流量仍走旧端口，问题可能在别处，按下一节的网络排查处理。
2. Codex CLI 启动时可能用 Codex Home 下 `.env` 中的非 `CODEX_` 键（例如写死的
   `HTTP_PROXY`）覆盖继承的环境变量。Guard 不读取、不修改该文件；请在本地自行检查
   其中的代理键。不要把 `.env` 或任何认证文件的内容粘贴到 issue 里。

## Desktop 启动后无法联网

Guard 不检测代理可用性，也不验证流量是否经过代理。请在代理软件中确认：

1. HTTP/Mixed 端口与配置一致；
2. 代理软件正在运行；
3. 当前节点和路由可用。

若能登录但对话卡住或流式输出中断，还应确认代理、防火墙或安全网关允许 HTTPS 与 WebSocket
Upgrade，且不会改写 TLS 或过早关闭长连接。ChatGPT 使用 `wss://ws.chatgpt.com`，Codex
使用 `wss://chatgpt.com/`。Guard 不会为这些情况发起探测。

ChatGPT Voice 可能优先使用 UDP，因此 HTTP/Mixed 代理环境不保证覆盖它。然后完全退出
ChatGPT Desktop，再通过 Guard 重新启动。

## 配置来自旧版本或代理端口变更

旧版本配置不会迁移。Managed 时代（v3）的配置会显示明确提示：该格式不受支持，请在
Guard 中按 `C` 重建本机代理配置（旧 Managed 数据不会被自动删除）。其他无效配置同样
进入修复界面；在保存有效配置之前，Launch 会持续被阻止，关闭错误提示也不会放行。

执行 `codex-proxy-guard init-config --force` 生成当前最小配置；该命令会覆盖现有文件，
应先记录需要保留的代理端点。

代理软件不限于 v2rayN，但必须提供本机 HTTP/Mixed 端口。若实际端口为 `7890`，可执行：

```powershell
codex-proxy-guard init-config --force --proxy-host 127.0.0.1 --proxy-port 7890
```

或手动更新 `[proxy]` 的 `host` 与 `port`。SOCKS-only 端口不能直接使用。

## Guard 退出后 Desktop 仍在运行

这是预期行为。Guard 只负责启动时注入环境，不托管 Desktop 生命周期；Guard 退出时的
清理只针对自己创建的短命子进程。
