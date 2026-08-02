# Security Model

## 强制边界

- 代理 scheme 必须为 `http`；
- 代理 host 必须是 `localhost` 或 loopback IP；
- 不接收代理用户名、密码或远程代理地址；
- 不写 Windows 系统代理、注册表代理或 Codex 用户配置；
- 不读取 Token、Cookie、OAuth、认证文件、浏览器数据或 Credential Manager；
- 不连接 Codex app-server 或 Desktop 私有 IPC；
- 不抓包、不解密 TLS、不保存网络内容；
- 不发现、启动、终止或配置 v2rayN；
- 不强制终止 Desktop；
- 不把环境变量注入宣称为全流量代理或强制网络策略；
- 所有外部错误在 TUI/CLI 展示前脱敏；
- Guard 退出只取消自身工作，不终止外部进程。

## 信任边界

配置文件由当前用户控制，但仍必须通过 loopback HTTP 校验。APPX 与 override 路径在
启动前必须是现存普通文件。APPX 清单入口必须是安装目录内的相对路径，canonicalize 后不得
逃逸至安装目录外；清单缺失时仅使用固定的受控后备可执行文件名。Desktop “已运行”只由与已
发现可执行文件路径相同、具有有效启动时间且不是 Chromium `--type=` 子进程的根进程证明。

## 不做网络判断

Guard 不访问配置的代理端口，也不访问 OpenAI 域名。代理失效时 Desktop 仍会启动，
随后由应用自身报告网络错误。这是有意的职责边界，不是健康检查遗漏。

`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` 仅传递给新启动的进程树。它们不构成 VPN、
透明代理或防泄漏控制：Guard 不接管 DNS、UDP、系统服务或应用后续以其他路径建立的连接。
尤其不应把 ChatGPT Voice 等可能使用 UDP 的流量视为已经被 HTTP 代理覆盖。
