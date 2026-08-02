# Architecture

## 单一职责

Codex Proxy Guard 仅为新启动的 ChatGPT Desktop（Chat、Work 和 Codex）进程树注入本机
HTTP/Mixed 代理环境。它不管理代理软件、不检查网络质量，也不接管 Desktop 生命周期。

```text
用户启动或按 Enter
→ 校验当前配置
→ 发现 Desktop
→ 获取跨进程启动锁
→ 确认 Desktop 根进程未运行
→ 注入代理环境
→ spawn Desktop
→ 返回 LaunchReceipt
```

## Crate 边界

- `proxy-guard-core`：配置、领域状态、Action/Effect/TaskResult、reducer 与脱敏。
- `proxy-guard-windows`：APPX 发现、Desktop 根进程检测、启动锁、环境注入与进程启动。
- `codex-proxy-guard`：CLI、单屏 TUI、effect dispatch 与启动编排。

所有会改变外部状态的操作都遵循：

```text
Action → candidate reduce → authorize → commit → dispatch → TaskResult
```

任意时刻只允许一个前台操作。

## 配置与 TUI

配置文件只接受当前 schema；旧版本和未知字段会被拒绝，不会迁移或静默忽略。无子命令启动
时，若配置无效，应用进入配置修复界面而不是退出：按 `C` 打开编辑器，使用 `Ctrl-U` 清空
当前字段，`Tab`/方向键切换 host 与 port，`Enter` 保存，`Esc` 放弃。

保存采用 effect/TaskResult 往返：先校验 loopback HTTP/Mixed 端点，再异步写入；失败时保留
编辑内容和错误提示。保存期间不接受第二个前台操作。

## 启动互斥

从进程快照到 spawn 期间，Guard 持有当前用户临时目录内的排他文件锁。Windows 使用
`share_mode(0)`，防止两个 Guard 同时通过“未运行”检查。spawn 成功后仅记录短暂时间戳以
覆盖新进程尚未出现在快照中的竞态；Guard 退出绝不终止 Desktop。

## APPX 发现与环境

优先使用 `codex.executable_override`，其次使用当前运行期缓存，最后通过受限且有超时的
PowerShell 查询当前 ChatGPT Desktop 与 ChatGPT Classic 的已知 APPX 包。发现阶段只从同一
产品内选择最高版本；选择阶段固定优先当前 ChatGPT Desktop，Classic 仅作后备，因此不会把
两个独立产品的版本号混在一起比较。

PowerShell 返回每个候选包的包名、版本、架构、安装目录和 APPX 清单入口。Rust 端校验包名，
要求清单入口为相对路径，并在 canonicalize 后仍位于安装目录内。清单入口不存在时，才尝试
受控的 `app\ChatGPT.exe` 与 `app\Codex.exe` 后备路径。`DesktopAppInfo` 保留产品类型、
架构及发现来源，CLI JSON 回执与 TUI 都展示这些非敏感元数据。

新进程树继承现有环境，并仅覆盖：

```text
HTTP_PROXY / HTTPS_PROXY
http_proxy / https_proxy
NO_PROXY / no_proxy
```

同时移除 `ALL_PROXY` / `all_proxy`。不会清空完整环境、修改 Windows 系统代理或编辑 Codex
用户配置。

这只是环境注入，不是网络强制或健康判断；Guard 不检查 HTTPS、WebSocket、DNS 或 UDP 是否
实际经过代理，也不为这些路径添加任何探测。
