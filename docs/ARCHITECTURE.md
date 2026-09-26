# Architecture

## 单一职责

Codex Proxy Guard 是最小化 Windows 启动器：使用用户配置的本机 HTTP/Mixed 代理启动
ChatGPT Desktop（Chat、Work 和 Codex），并在必要时通过 Codex 官方生命周期命令刷新旧
后台 daemon，使新进程环境真正生效。它不管理代理软件、不检查网络质量，也不接管
Desktop 生命周期。

```text
用户 Launch
→ 校验配置
→ 获取跨进程启动锁
→ 校验 Guard 非管理员运行
→ 发现 Desktop
→ 确认 Desktop 未运行
→ 尝试解析 Codex CLI
→ 通过官方 daemon stop 清理旧后台服务
→ 注入 HTTP_PROXY / HTTPS_PROXY / NO_PROXY
→ spawn Desktop
→ Desktop/Codex 自行建立新的后台 daemon
→ LaunchReceipt
```

Guard does not own the Codex daemon. Guard does not start, restart, update,
bootstrap or monitor the daemon.

## Crate 边界

- `proxy-guard-core`：配置、领域状态（含 `DaemonPreparation`）、Action/Effect/TaskResult、
  reducer 与脱敏。
- `proxy-guard-windows`：APPX 发现、Desktop 根进程检测、启动锁、权限检查、Codex CLI
  解析与 daemon 兼容适配、环境注入与进程启动。
- `codex-proxy-guard`：CLI、单屏 TUI、effect dispatch 与启动编排。

所有会改变外部状态的操作都遵循：

```text
Action → candidate reduce → authorize → commit → dispatch → TaskResult
```

任意时刻只允许一个前台操作。

## 配置与 TUI

配置文件只接受当前 schema；旧版本和未知字段会被拒绝，不会迁移或静默忽略。
`codex.cli_executable_override` 是 V2 schema 的可选扩展：已有 V2 配置缺省该字段时按空值
处理。无子命令启动时，若配置无效，应用进入配置修复界面而不是退出：按 `C` 打开编辑器，
使用 `Ctrl-U` 清空当前字段，`Tab`/方向键切换 host 与 port，`Enter` 保存，`Esc` 放弃。

保存采用 effect/TaskResult 往返：先校验 loopback HTTP/Mixed 端点，再异步写入；失败时保留
编辑内容和错误提示。保存期间不接受第二个前台操作。

TUI 保持单屏形态，不新增 daemon 管理界面。Launch 成功后的 status message 按 daemon
准备结果展示一句补充说明（未运行 / 已刷新 / 生命周期 API 不可用）。

## 启动互斥

从进程快照、daemon 准备到 spawn，Guard 持有当前用户临时目录内的排他文件锁。Windows 使用
`share_mode(0)`，防止两个 Guard 同时通过“未运行”检查，也防止并发实例对同一 daemon
执行 stop 与 spawn 的竞态。spawn 成功后仅记录短暂时间戳以覆盖新进程尚未出现在快照中的
竞态；Guard 退出绝不终止 Desktop。锁的生命周期覆盖 Desktop 进程检查 → daemon
preparation → 环境注入 → Desktop spawn 全程。

## 权限检查

Codex 0.157+ 的共享后台服务拒绝由提升进程启动（官方实现检查进程 token 的
`TokenElevation`）。Guard 采用与 Codex 一致的 Win32 检查（`OpenProcessToken` /
`GetTokenInformation` / `TokenElevation`，不调用 `whoami` 或 `net session`）：若 Guard
自身以管理员身份运行，Launch 立即被阻止并提示 `ELEVATED_LAUNCH_UNSUPPORTED`。Guard 不
自动 UAC、不创建低权限 token、不做任何降权伪装。

## Codex daemon 兼容适配

Codex 0.157+ 默认启用常驻后台 daemon；daemon 保留其启动时继承的环境变量，新 Desktop
进程无法改变它。因此纯进程树注入在旧 daemon 存在时会失效。适配层只做一件事：

```text
启动前执行官方公开命令 codex app-server daemon stop
→ 旧 daemon 由 Codex 官方 CLI 正常关闭
→ Desktop 以注入的代理环境启动
→ Codex 自行创建新的 daemon 并继承新环境
```

Codex CLI 解析顺序（不做全盘扫描、不用模糊文件搜索）：

1. `codex.cli_executable_override`（存在才生效）；
2. `%CODEX_HOME%\packages\app-server-daemon\current\bin\codex.exe`；
3. `%CODEX_HOME%\packages\standalone\current\bin\codex.exe`；
4. CODEX_HOME 缺省为 `%USERPROFILE%\.codex`；
5. `where.exe codex.exe`（受限、超时 5 秒）。

`daemon stop` 子进程：stdin 为 null、stdout/stderr 管道化且上限 64 KiB、
`kill_on_drop(true)`、总超时 75 秒（覆盖 daemon 60 秒 graceful shutdown）。超时只终止
CLI 子进程，绝不直接杀 daemon，并以 `CODEX_DAEMON_STOP_TIMEOUT` 阻止本次 Launch。

结果映射：

- 输出 `notRunning` → `DaemonPreparation::NotNeeded`，直接继续；
- 输出 `stopped` → `DaemonPreparation::Stopped`，继续启动；
- 找不到 CLI、旧版 Codex 报 unrecognized subcommand / unknown command /
  daemon command unsupported → `DaemonPreparation::LifecycleUnavailable`，不阻止旧版
  Codex 启动，按传统环境注入进行；
- CLI 存在但 stop 明确失败、输出无法识别或 daemon 仍为 running →
  `CODEX_DAEMON_STOP_FAILED` 阻止 Launch，不在可能存在 stale daemon 的情况下启动。

所有 stderr/stdout 摘要经 `redact_text()` 脱敏且截断后才展示。`LaunchReceipt` 额外记录
`daemon_preparation`，便于排查；不包含 daemon socket、auth、token、Codex Home 或本地
安装路径。

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
用户配置（包括 `~/.codex/config.toml` 与 `~/.codex/.env`，不会写
`features.daemon_auto_start` 之类的开关）。

这只是环境注入，不是网络强制或健康判断；Guard 不检查 HTTPS、WebSocket、DNS 或 UDP 是否
实际经过代理，也不为这些路径添加任何探测。
