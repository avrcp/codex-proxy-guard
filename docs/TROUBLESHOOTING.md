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

## `APPX_DISCOVERY_PROTOCOL_INVALID` / `APPX_DISCOVERY_PROTOCOL_UNSUPPORTED`

发现脚本的 JSON 契约损坏：成功执行但没有 envelope、envelope 结构不合法、或 schema
版本不是当前支持的版本（错误中会带实际版本号）。这是协议故障，不是"未安装"——
Guard 不会把它当成 CODEX_NOT_INSTALLED，更不会退化为裸 EXE 启动。可用
`powershell -File resources/appx-discovery.ps1` 人工执行同一份生产脚本核对输出；
若输出异常请连同 schema_version、records 数量与 applications 的 JSON 类型一并记录。

## `ACTIVATION_ONLY_UNSUPPORTED`

`launch --activation-only` 是注册应用激活路径的身份对照诊断；普通未打包 EXE 没有激活可对照。
该标志不会静默退化为带环境的普通启动。

## `APPX_ACTIVATION_FAILED`（含 HRESULT）

Windows 拒绝了应用激活请求（错误中带十六进制 HRESULT）。Guard 不会退回到裸 EXE、
调试上下文或管理员方式。先用 `scripts/inspect-package-launch.ps1` 只读核对注册入口；
若最近刚升级包，按 `R` 刷新后重试。

## `APPX_ACTIVATION_OUTCOME_UNKNOWN`

激活请求已提交但结果未知（取消、超时或 worker 未回执）。Windows 可能已经启动应用；
Guard 不会自动重试，也不会杀掉目标。先按 `R` 刷新查看 Desktop 是否在运行，再决定
是否需要下一次显式启动。

## `APPX_TARGET_EXITED_EARLY`

激活曾返回 PID，但目标在观察窗口内退出（错误中含退出码）。保留该退出码；这通常是
应用自身初始化失败，不是身份缺失。

## `APPX_METADATA_INCOMPLETE` / `APPX_APPLICATION_AMBIGUOUS`

包注册元数据不足，或多个 Application 无法唯一确定桌面主入口。Guard 不会任选首个
EXE。可用 `scripts/inspect-package-launch.ps1` 只读检查本机包与 Application；该脚本
不会启动应用或读认证内容。不要把包目录、家庭目录等本机路径原样贴到公开报告。

## `APPX_IDENTITY_MISSING` / `APPX_IDENTITY_MISMATCH` / `APPX_AUMID_MISSING` / `APPX_AUMID_MISMATCH` / `*_QUERY_FAILED`

分别表示实际目标进程没有包身份/应用身份、身份与本轮所选注册条目不一致、或 Windows
查询未能确认（查询失败与"没有身份"是不同的事实）。即使进程曾被创建，这些结果都不是
启动成功。不要自动再次启动，也不要以修改代理端口代替身份排查；记录错误码、目标 PID
与包版本后再定位。若身份正确但仍见"无程序包标识符"弹窗，用
`scripts/inspect-package-launch.ps1 -ProcessId <弹窗进程PID>` 定位弹窗归属——它可能
来自另一个进程（应用内部重启的子进程、旧实例），不能把根进程身份结论套用到所有
后代进程。

## `PROXY_BYPASS_UNSUPPORTED`

`proxy.no_proxy` 中存在无法等价映射到 Chromium bypass 列表的条目（域名、通配符、
CIDR 等）。Guard 拒绝静默丢弃规则。请把该条目从 `no_proxy` 中移除（默认的
`localhost`、`127.0.0.1`、`::1` 会自动映射）。

## `BACKEND_PROXY_CONSENT_REQUIRED` / `BACKEND_PROXY_SCOPE_UNCONFIRMED`

后端 `.env` 代理块未授权，或启用了授权但没有绑定已确认的绝对 Codex Home。在 TUI
按 `B` 查看确切 Home 并确认（默认取消）。Guard 不会猜测 Home，也不会用自己的
`CODEX_HOME` 代替确认。

## `BACKEND_PROXY_REQUIRED_FOR_REPAIR`

已注册 Desktop 的修复启动（`D` / `--refresh-codex-daemon`）需要后端代理块先行授权：
应用激活无法给 Codex 后端注入环境变量，未授权就停止共享服务只会中断任务而建立不了
任何新配置。先按 `B` 授权（或改用普通启动）。未打包目标不受影响。共享服务未被触碰。

## `INVALID_LAUNCH_OPTIONS`

`--activation-only` 与 `--refresh-codex-daemon` 互斥：为一个明确不提交代理的诊断而
中断共享服务没有合理意义。CLI 直接拒绝该组合；领域层同样校验（TUI/测试/未来入口
构造的选项也会被拒）。未执行任何操作。

## `BACKEND_PROXY_CONFIG_CONFLICT`

`.env` 中 Guard 标记块之外已有 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`（含大小写变体）
或 `ALL_PROXY`。Guard 绝不抢占：错误只列出键名（不回显值），请手工整理后再授权。

## Coverage 状态行的含义

`Coverage` 显示的是授权 Home `.env` 块的**真实磁盘状态**（`R` 刷新时重新检查）：
`setup required`（未授权，黄色）、`pending sync`（已授权但块未写入，黄色）、
`current`（块与当前配置一致，绿色）、`stale · syncs on launch`（代理配置已改、
下次启动同步，黄色）、`conflict` / `needs attention`（红色）。黄色是需要你注意的
真实状态，不会为了"全绿"自动写 `.env`。

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
后台服务零副作用，也不会因为本机没有 Codex CLI 而失败。已注册应用通过 Windows
注册入口原生激活。只有你显式授权的修复启动
（TUI 按 `D` 再按 `Y`，或 `launch --refresh-codex-daemon`）才会执行
`codex app-server daemon stop`——该命令可能中断同一 Codex Home 下其他 CLI / IDE /
远程客户端正在执行的任务，因此必须逐次确认，授权不会保存。包身份问题不需要也不
应该先停止 daemon。

## `CODEX_CLI_UNAVAILABLE` / `CODEX_CLI_OVERRIDE_INVALID` / `CODEX_HOME_INVALID`

显式修复启动找不到可用的官方 Codex CLI，或配置的 override / `CODEX_HOME` 无效。这些
错误不会静默降级为普通启动。

- 未安装 CLI：普通路径不需要 CLI；包身份阻断需按上面的包启动说明处理；
- override 无效：`codex.cli_executable_override` 必须是现存绝对路径的 native
    `codex.exe`（不支持 `.cmd`/`.ps1` shim）；
- 显式 `CODEX_HOME` 必须是现存目录；未设置时使用用户目录下的默认 `.codex`。

## `CODEX_DAEMON_UNSUPPORTED`

已解析的 Codex CLI 不支持 `app-server daemon` 生命周期命令（常见于较旧版本）。修复
启动被阻止；普通路径仍不会操作 daemon，但已注册包可能因包身份要求而阻断。

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
   `HTTP_PROXY`）覆盖继承的环境变量。让后端走代理的受管方式是在 TUI 按 `B` 授权
   Guard 的 `.env` 代理块（只管理自己的标记块）；若你想手工维护，请自行检查其中的
   代理键——若手工键与 Guard 块并存，Guard 会报告 `BACKEND_PROXY_CONFIG_CONFLICT`
   并拒绝写入。不要把 `.env` 或任何认证文件的内容粘贴到 issue 里。

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
