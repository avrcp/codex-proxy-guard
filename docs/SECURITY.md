# Security Model

## 强制边界

- 代理 scheme 必须为 `http`；
- 代理 host 必须是 `localhost` 或 loopback IP；
- 不接收代理用户名、密码或远程代理地址；
- 不写 Windows 系统代理、注册表代理或 Codex 用户配置；
- 不读取 Token、Cookie、OAuth、认证文件、浏览器数据或 Credential Manager；
- 不抓包、不解密 TLS、不保存网络内容；
- 不发现、启动、终止或配置 v2rayN；
- 不强制终止 Desktop；
- 不把环境变量注入宣称为全流量代理或强制网络策略；
- 所有外部错误在 TUI/CLI 展示前脱敏；
- Guard 退出只取消自身工作，不终止外部进程。

## Codex daemon 兼容边界

允许的唯一 daemon 交互是：

```text
codex app-server daemon stop
```

并且仅限：

- Desktop 启动前的准备阶段；
- 通过本地解析出的官方 Codex CLI（override / CODEX_HOME 包布局 / `where.exe`）执行；
- stdin 为 null、输出管道化并设 64 KiB 上限、超时 75 秒、可取消、kill_on_drop。

禁止：

- `daemon start`、`daemon restart`、`daemon update`、`daemon bootstrap` 或任何形式的
  remote-control；
- 连接 app-server socket、JSON-RPC、私有 IPC、daemon PID file 或 daemon settings；
- 读取 daemon 私有状态或任何认证数据；
- 直接终止 Codex 进程（`TerminateProcess` / `taskkill` / PID 强杀 / Job Object）；
- 修改 `~/.codex/config.toml`、`~/.codex/.env`，或写入 `features.daemon_auto_start` 之类
  的开关来对抗 daemon 架构。

daemon stop 的唯一目的是在 Desktop 启动前丢弃旧 daemon 持有的过期进程环境，让新启动的
Desktop 在注入的代理环境下建立新 daemon。

## 权限边界

Guard 不以管理员身份启动 Desktop：Codex 0.157+ 的共享后台服务要求非提升进程。Guard 用
与 Codex 官方一致的 `TokenElevation` 检查；发现自身提升运行时直接阻止 Launch
（`ELEVATED_LAUNCH_UNSUPPORTED`），不自动 UAC、不创建低权限 token、不做降权伪装。

## 信任边界

配置文件由当前用户控制，但仍必须通过 loopback HTTP 校验。APPX 与 override 路径在
启动前必须是现存普通文件。APPX 清单入口必须是安装目录内的相对路径，canonicalize 后不得
逃逸至安装目录外；清单缺失时仅使用固定的受控后备可执行文件名。`cli_executable_override`
只在指向现存文件时生效，否则回退到 CODEX_HOME 与 PATH 解析。Desktop “已运行”只由与已
发现可执行文件路径相同、具有有效启动时间且不是 Chromium `--type=` 子进程的根进程证明。

## 不做网络判断

Guard 不访问配置的代理端口，也不访问 OpenAI 域名。代理失效时 Desktop 仍会启动，
随后由应用自身报告网络错误。这是有意的职责边界，不是健康检查遗漏。

`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` 仅传递给新启动的进程树。它们不构成 VPN、
透明代理或防泄漏控制：Guard 不接管 DNS、UDP、系统服务或应用后续以其他路径建立的连接。
尤其不应把 ChatGPT Voice 等可能使用 UDP 的流量视为已经被 HTTP 代理覆盖。
