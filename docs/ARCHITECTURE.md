# Architecture

## 单一职责

Codex Proxy Guard 是最小化 Windows 启动器：使用用户配置的本机 HTTP/Mixed 代理启动
ChatGPT Desktop（Chat、Work 和 Codex）。普通启动对 Codex 共享后台服务零副作用；
仅在用户单次显式授权的修复启动中，Guard 通过官方生命周期命令停止共享服务。它不管理
代理软件、不检查网络质量，也不接管 Desktop 生命周期。

普通启动（默认）：

```text
用户 Launch（Enter / launch）
→ 校验配置
→ 权限检查（非提升；查询失败也阻断）
→ 发现 Desktop（每次启动重新发现，不信任显示缓存）
→ 获取跨进程启动锁
→ 确认 Desktop 未运行
→ 注入进程级代理环境
→ spawn Desktop
→ 返回事实回执（daemon_preparation = skipped）
```

显式修复启动（`D` + `Y` 确认，或 `launch --refresh-codex-daemon`）：

```text
用户单次确认可能中断共享任务
→ 同一套启动前检查与启动锁
→ 可信来源解析 Codex CLI 与目标 CODEX_HOME
→ 官方 codex app-server daemon stop（总预算 720 秒，可取消）
→ 再检查：取消、可执行文件、Desktop 运行状态
→ 注入环境 → spawn Desktop → 回执（stopped / not_needed）
```

两条入口共用同一条启动管线与同一套检查；修复授权是单次调用意图，不持久化到配置。

Guard does not own the Codex daemon. Guard does not start, restart, update,
bootstrap or monitor the daemon.

## Crate 边界

- `proxy-guard-core`：配置、领域状态（`DaemonPreparation`、`ConfigReadiness`、
  `LaunchOptions`）、Action/Effect/TaskResult、reducer 与脱敏。
- `proxy-guard-windows`：APPX 发现、Desktop 根进程检测、启动锁、权限查询、Codex CLI
  解析与显式修复的 daemon stop、环境注入与进程启动。
- `codex-proxy-guard`：CLI、单屏 TUI、effect dispatch 与启动编排。

所有会改变外部状态的操作都遵循：

```text
Action → candidate reduce → authorize → commit → dispatch → TaskResult
```

任意时刻只允许一个前台操作。

## 配置可用性与 TUI

配置文件只接受当前 schema；旧版本和未知字段会被拒绝，不会迁移或静默忽略。
`codex.cli_executable_override` 是 V2 schema 的可选扩展。

无效配置进入 `ConfigReadiness::RepairRequired`：内存默认值只作为编辑表单初值，不可用
来启动。关闭错误提示、刷新、取消编辑、保存失败都不能解除阻断；只有成功保存有效配置
（或重新读取到有效配置）才会回到 Ready。v3（Managed）配置会得到明确提示：不支持、需
显式重建本机代理配置，不会把 v3 默认端口当成用户配置，也不触碰旧 Managed 数据。

无子命令启动时若配置无效，应用进入配置修复界面而不是退出：按 `C` 打开编辑器，使用
`Ctrl-U` 清空当前字段，`Tab`/方向键切换 host 与 port，`Enter` 保存，`Esc` 放弃。host
校验与 URL 构造使用同一规则（拒绝前后空白），不会出现“校验通过但拼出坏 URL”。

TUI 保持单屏形态。修复启动是低频入口：`D` 打开模态确认（说明可能中断同一 Codex Home
的共享任务，默认取消），仅 `Y` 单次确认、`N`/`Esc` 取消；刷新或编辑配置会关闭确认。
Launch 成功后的 status message 只陈述观测事实（进程已创建、环境已传入；修复路径追加
共享服务已停止/未运行）。

## 启动互斥与任务收尾

启动锁从 Desktop 运行检查、daemon 准备一直持有到 spawn 完成，协调的是 Guard 实例之间
的启动，不能阻止外部 Codex 客户端同时操作 daemon，也不是跨产品原子事务。Windows 使用
`share_mode(0)`；spawn 成功后记录短暂时间戳覆盖新进程尚未出现在快照中的竞态。

Guard 明确区分两类子进程：

```text
Guard 自己创建的短命子进程（PowerShell 发现、lifecycle CLI）：
    输出 64 KiB 上限、总预算超时、取消、kill_on_drop；超时/取消时终止并有界回收。

Desktop、共享 daemon、其他客户端、代理软件：
    不按名称/PID 强杀，不递归终止进程树，不绑定 kill-on-close Job。
```

取消契约：管线入口、取锁后、每个异步外部操作前后、helper spawn 前、最终 spawn 前都
检查取消；最后一次检查与同步 spawn 之间没有 `await`——已观测到取消就不再发起新的
spawn，spawn 已发生则绝不因取消杀掉 Desktop。TUI 退出先 cancel 再有界 join 前台任务；
CLI 启动把 Ctrl-C 转成取消后等待清理退出。退出阶段不消费迟到的任务结果。

## 权限检查

Codex 0.157+ 的共享后台服务拒绝由提升进程启动。Guard 用与 Codex 官方一致的 Win32 检查
（`OpenProcessToken` / `GetTokenInformation` / `TokenElevation`，RAII 管理 handle）。
查询有三种结果：非提升 → 允许；提升 → `ELEVATED_LAUNCH_UNSUPPORTED`；查询失败或结果
长度异常 → `ELEVATION_QUERY_FAILED`，绝不把失败解释成非提升。TUI 启动时前置显示原因，
最终执行前由管线重新检查。整个 Guard 选择普通权限运行是本项目的保守策略；Guard 不做
自动 UAC、降权 token 或权限伪装。

## 显式修复的 daemon 兼容

Codex CLI 与 Desktop 是独立产品；修复路径的对象是“已解析 Codex Home 中的共享服务”，
不推断任意 Desktop 一定使用它。CLI 解析（仅显式修复时执行）：

1. `codex.cli_executable_override`：现存绝对路径的 native `.exe`；无效即
   `CODEX_CLI_OVERRIDE_INVALID`，不换源、不执行 `.cmd`/`.ps1` shim；
2. 显式 `CODEX_HOME`：必须存在且为目录（相对路径按启动工作目录 pin 成绝对路径，仅向
   该操作的子进程传递），不会回退默认目录，也不会额外扫描另一个 Home；
3. 包布局候选：`packages/app-server-daemon/current/bin/codex.exe`、
   `packages/standalone/current/bin/codex.exe`（含 legacy 平铺
   `packages/standalone/current/codex.exe`）；
4. PATH：仅绝对目录，跳过空项、`.`、相对项与当前目录，不使用 `where.exe`。

解析只证明“受控来源选择”，不证明 OpenAI 签名。

`daemon stop` 是一个完整操作：两条管道各有 64 KiB 硬上限（读到上限+1 字节即失败，先于
任何解析或降级分类）、严格 UTF-8、恰好一个 JSON 对象、只接受精确的 `stopped` /
`notRunning`（未知字段可忽略，未知状态即失败）。总预算 720 秒（官方宽限期 0–300 秒加
生命周期锁等待的保守项目上限，不宣称覆盖任意锁竞争），取消全程可用；超时终止并回收
本次创建的 CLI 子进程，绝不直接杀 daemon。失败语义：

| 场景 | 结果 |
| --- | --- |
| 找不到 CLI | `CODEX_CLI_UNAVAILABLE`，阻断（提示可普通启动） |
| override 无效 / Home 无效 | `CODEX_CLI_OVERRIDE_INVALID` / `CODEX_HOME_INVALID`，阻断 |
| CLI 不支持 daemon 命令 | `CODEX_DAEMON_UNSUPPORTED`，阻断 |
| stop 失败 / 超限 / 非法输出 / 超时 | `CODEX_DAEMON_STOP_FAILED` / `_TIMEOUT`，阻断 |
| stop 成功后取消 | 阻断，但保留“daemon 已停止”事实 |
| stop 成功后 Desktop spawn 失败 | 错误中保留“daemon 已停止”事实，不自动重启 |

所有 stderr/stdout 摘要先脱敏、去控制字符、再截断；回执不含原始 CLI 输出、socket
路径、完整命令行或用户 Home。

## APPX 发现与环境

优先使用 `codex.executable_override`，PowerShell 查询固定使用系统目录下 Windows
PowerShell 的绝对路径（不解析 PATH，不接受工作目录同名文件），超时 15 秒、输出上限
64/128 KiB。发现阶段只从同一产品内选择最高版本；选择阶段固定优先当前 ChatGPT Desktop，
Classic 仅作后备。Rust 端校验包名，要求清单入口为相对路径，并在 canonicalize 后仍位于
安装目录内（该 containment 校验与路径身份比较是两回事，不互相替代）。清单入口不存在
时才尝试受控的 `app\ChatGPT.exe` 与 `app\Codex.exe` 后备路径。

进程身份比较把普通路径与 extended-length（`\\?\C:\...`、`\\?\UNC\server\share\...`）
写法规范化后做 ASCII 忽略大小写比较，等价写法不漏检、别处同名 exe 不误认；无法读取
可执行路径但名称与目标一致的候选报告 `Unknown`（不能确认未运行），不假装成已停止，
无关进程的不可读不影响全局。显式修复在 stop 等待窗口后、spawn 前于锁内重新检查取消、
可执行文件与 Desktop 运行状态；真正的 Launch 每次重新发现 APPX，不依赖 TUI 显示用的
缓存。

新进程树继承现有环境，并仅覆盖：

```text
HTTP_PROXY / HTTPS_PROXY
http_proxy / https_proxy
NO_PROXY / no_proxy
```

同时移除 `ALL_PROXY` / `all_proxy`。不会清空完整环境、修改 Windows 系统代理或编辑
Codex 用户配置（包括 `~/.codex/config.toml` 与 `~/.codex/.env`）。若修复时 pin 了显式
相对 `CODEX_HOME`，同一绝对值只传给该次 CLI helper 与 Desktop 子进程，保证两者作用域
一致，且不修改 Guard 自身环境。

这只是环境注入，不是网络强制或健康判断；Guard 不检查 HTTPS、WebSocket、DNS 或 UDP 是否
实际经过代理，也不为这些路径添加任何探测。
