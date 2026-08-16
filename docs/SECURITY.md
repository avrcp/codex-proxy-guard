# Security Model

## 强制边界

- 代理 scheme 必须为 `http`；
- 代理 host 必须是 `localhost` 或 loopback IP；
- 不接收代理用户名、密码或远程代理地址；
- 不写 Windows 系统代理、注册表代理或 Codex 用户配置；
- 不读取 Token、Cookie、OAuth、认证文件、浏览器数据；
- 不连接 Codex app-server 或 Desktop 私有 IPC；
- 不抓包、不解密 TLS、不保存网络内容；
- 不发现、启动、终止或配置 v2rayN；
- 不强制终止 Desktop；
- 不把环境变量注入宣称为全流量代理或强制网络策略；
- 所有外部错误在 TUI/CLI 展示前脱敏；
- Guard 退出只取消自身工作，不终止外部进程。

## Managed Mode 扩展边界

Managed Mode 增加订阅管理、Guard 自有的 sing-box sidecar，以及受控的 HTTPS/Geo
benchmark，但交付物仍然是 loopback HTTP proxy：

- 订阅 URL 只保存于 Windows Credential Manager 的 `CodexProxyGuard.Subscription`
  命名空间下，键为 `<SubscriptionId>`；从不序列化、不写日志、不显示、不出现在
  `--json` 输出或 benchmark 文件中；
- 只按已知 `SubscriptionId` 读写自己的 secret，禁止枚举或读取其他 Credential
  Manager 条目；
- 订阅下载走主机真实网络（system proxy 关闭），不经过当前 managed 节点；
- 只导入名称可归类为 JP/SG/US 的节点；其他地区不保存；
- 节点真实出口国必须与 hint 一致，country mismatch 永远无法成为 winner；
- 节点 outbound 不允许用户指定 `tag`，拒绝 `direct/block/dns/selector/urltest`；
- sing-box 只使用 `127.0.0.1:<ephemeral>` mixed inbound，`set_system_proxy=false`，
  启动前必须 `sing-box check` 通过；Guard 持有 process handle，Drop/退出回收整棵
  进程树；
- 不读取 ChatGPT/Codex token、cookie 或认证文件，不调用私有 API，不用真实 prompt
  测速。

## 信任边界

配置文件由当前用户控制，但仍必须通过 loopback HTTP 校验。APPX 与 override 路径在
启动前必须是现存普通文件。APPX 清单入口必须是安装目录内的相对路径，canonicalize 后不得
逃逸至安装目录外；清单缺失时仅使用固定的受控后备可执行文件名。Desktop “已运行”只由与已
发现可执行文件路径相同、具有有效启动时间且不是 Chromium `--type=` 子进程的根进程证明。

Managed Mode 的订阅元数据与节点文档位于 `%APPDATA%/codex-proxy-guard/managed/`，
只通过 ID 派生目录读写，canonicalize 后不得逃逸到 managed 根之外；订阅 URL 永远不落盘。

## 不做网络判断

External Mode 下 Guard 不访问配置的代理端口，也不访问 OpenAI 域名。代理失效时
Desktop 仍会启动，随后由应用自身报告网络错误。

Managed Mode 下的 benchmark 只针对 `https://chatgpt.com/` 做受控、低频、限时的
HEAD 与 Geo 查询，用于在 JP/SG/US 之间选节点，不做账号/服务资格判断。

`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` 仅传递给新启动的进程树。它们不构成 VPN、
透明代理或防泄漏控制：Guard 不接管 DNS、UDP、系统服务或应用后续以其他路径建立的连接。
尤其不应把 ChatGPT Voice 等可能使用 UDP 的流量视为已经被 HTTP 代理覆盖。
