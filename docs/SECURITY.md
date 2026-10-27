# Security Model

## 发布校验边界（release verification）

发布门禁只声明它亲自验证过的事实：EXE 在签名后不再被改动，摘要与
manifest/transport ZIP/SHA256SUMS 逐字节绑定，导入表只含 Windows 系统 DLL，
PE 只按 x64/PE32+ 解析且越界即失败。CET 兼容仅以
`IMAGE_DEBUG_TYPE_EX_DLLCHARACTERISTICS`（type 20）扩展记录为准——普通头的
0x1000 位是 APPCONTAINER，GuardFlags 的 0x01000000 位属于其他命名空间，
二者都不是 CET 证据；记录截断按解析失败处理，不取猜测值。`cet_compat=true`
只表示映像标记，不表示运行时硬件 shadow stack 已启用；PE 的 CFG 标志证明
映像级事实，不证明每个对象文件都以 `/guard:cf` 编译——对象级覆盖来自共享
RUSTFLAGS（作用于每个 crate）。签名事实（publisher、签名者指纹、时间戳、验证策略）来自签名对象
与独立 `WinVerifyTrust`（VERIFY→CLOSE 配对，仅状态 0 视为有效；清理失败不会
被当成 Valid），本地化 signtool 文本永不解析。签名不能保证杀软永不拦截，
SmartScreen 对新签名文件仍可能出现信誉提示。

## Interactive frontend boundary

The TUI runs the engine in-process through the shared EngineSession: it
renders the session snapshot (the same view model every frontend consumes)
and maps keys to typed session requests. It cannot issue arbitrary engine
operations, edit TOML or `.env` itself, or bypass candidate validation. Backend
consent and repair each require a YES-typed modal bound to a single-use engine
confirmation token tied to the displayed scope and current configuration.
The plain line console is the same contract in line form.

Delivery judgment is an engine fact, not a frontend one: `LaunchOutcome`
carries the endpoint in the configuration the launch used and the endpoint the
receipt actually submitted, plus the divergence flag, so the TUI, the line
console and the headless CLI state one receipt in one wording instead of each
comparing strings locally. A failed launch says no delivery receipt was
recorded, which is not a claim that nothing was delivered.

Backend authorization is only ever re-read from the file it lives in. A repair
save that would have to re-create a missing configuration refuses to carry the
consent fields over from the last good read (`CONFIG_CONSENT_REPAIR_REQUIRED`,
file left absent): revoking or granting authorization stays an explicit,
single-use user act, and a positive `CONFIG_INVALID` read — never a read
failure — is the only evidence that licenses replacing unusable bytes. The
launch path runs the checks that need no file, handle or process before the
configuration lease and the cross-process startup lock are taken.

Guard runs asInvoker. The engine remains the final authority on elevation,
loopback proxy validation, package identity and repair gates. A
malformed/mismatched snapshot disables actions; no submitted operation is
auto-retried. A foreground operation that exceeds its session budget is
cancelled but the session stays busy until the effect's future is reaped —
the one-foreground-operation invariant holds end to end, nothing is detached.
Raw stderr is not displayed. Shutdown cancels cooperatively and
never kills Desktop or the shared daemon; only Guard's own short-lived helper
children (activation worker, repair CLI) are terminated and reaped, through
held handles, on timeout or cancellation.

## Guard-owned child processes

Every Guard-owned child is spawned by `runOwnedProcess` with separate
program/arguments (no shell, no cmd.exe, no PowerShell wrapper) and a
per-request budget. The pipes are created non-inheritable; the three
child-side ends are duplicated into the only inheritable handles Guard ever
holds and are additionally passed through a
`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, so exactly the standard handles cross
the process boundary even on hosts whose CreateProcess hook strips the
attribute list. stdin is delivered by a dedicated cancellable writer thread
(a child that never reads stdin cannot stall the budget machinery); the
verdict is re-checked after every worker thread is joined so a fast-exiting
flood cannot masquerade as a normal result. Custom environment blocks are
emitted ordinal-sorted by name with duplicates collapsed, exactly as
CreateProcessW requires. Only this direct child is ever terminated, never a
process tree and never a process matched by name.

## 强制边界

- 代理 scheme 必须为 `http`；
- 代理 host 必须是 `localhost` 或 loopback IP；
- 不接收代理用户名、密码或远程代理地址；
- 不写 Windows 系统代理或注册表代理，不改写 `~/.codex/config.toml`；除经显式授权的单一 `.env` 代理块外，不修改任何 Codex 用户文件；
- 不读取 Token、Cookie、OAuth、认证文件、浏览器数据或 Credential Manager；
- 不抓包、不解密 TLS、不保存网络内容；
- 不发现、启动、终止或配置 v2rayN；
- 不强制终止 Desktop；
- 不把环境变量注入宣称为全流量代理或强制网络策略；
- 所有外部错误在 console/CLI 展示前脱敏；
- Guard 退出只取消自身工作，不终止外部进程。

## 程序包身份边界

已注册 Desktop 只能通过 Windows 注册入口原生激活：以动态解析的 AUMID 调用
`IApplicationActivationManager::ActivateApplication`（`AO_NONE`）。绝不裸启动包目录内
EXE，不使用 `Invoke-CommandInDesktopPackage`、`IPackageDebugSettings` 调试模式、
`--no-sandbox`、调试端口或任何提权/降权技巧充当身份修复。

COM 调用由 Guard 自己的短命 worker 进程执行（同一 EXE 的隐藏
`internal-activate-package` 子命令；16 KiB stdin 请求 / 64 KiB stdout 回执，无命名管道、
无 nonce、无监听端口）。worker 不需要 OpenAI 包身份，也从不运行在包上下文中；它只接收
由本轮发现结果构建的 AUMID、预期包身份与白名单参数，拒绝其他任何字段。提交前取消
保证不发起激活；提交后取消/超时按 `APPX_ACTIVATION_OUTCOME_UNKNOWN` 报告，不自动重试，
不终止 Desktop。超时或取消只回收 Guard 自己的 worker。

Guard 持有句柄查询**实际目标进程**的 PackageFullName 与 AUMID，与本轮注册元数据精确
比较；缺失（`APPMODEL_ERROR_NO_PACKAGE/NO_APPLICATION`）、不匹配与查询失败分别报告，
Win32 状态码不冒充 HRESULT。映像路径、创建时间（区分新建/复用实例）与 TokenElevation
作为附加观测记录。返回的 PID 只是激活事实，不代表界面就绪或业务代理已生效。

## Codex daemon 兼容边界

普通启动对共享 daemon 零副作用：不解析 Codex CLI、不执行任何 daemon 命令、不依赖 CLI
已安装。允许的唯一 daemon 交互是显式修复启动（用户单次确认）中的：

```text
codex app-server daemon stop
```

并且仅限：

- 该次调用已获得用户明确授权（CLI `--refresh-codex-daemon` 或 console `R` + `YES`），授权不
  持久化到配置；
- 通过受约束的路径解析 Codex CLI 执行（不声称验证发行签名）：显式 override（现存绝对路径 native `.exe`，
  失效即错不换源）、显式 `CODEX_HOME`（必须存在，不回退默认目录、不跨 Home 扫描）、
  已知包布局、PATH 中的绝对目录（跳过当前目录与相对项，不使用 `where.exe`）；
- stdin 为 null、两条管道各 64 KiB 硬上限（超限先于解析失败）、严格 UTF-8、恰好一个
  JSON 对象、只接受精确 `stopped` / `notRunning`；
- 总预算 720 秒、取消全程可用；超时/取消时只终止并回收 Guard 自己创建的短命 CLI 子
  进程。

注意：官方 stop 命令本身也是破坏性操作——会中断同一 Codex Home 下其他 CLI / IDE /
远程客户端正在执行的任务；stop 成功也不代表新进程一定使用新代理（Codex CLI 启动时
可能用 Codex Home 下 `.env` 的值覆盖继承环境）。让后端走代理的唯一受管入口是下节的
授权代理块。

## Codex Home `.env` 授权代理块

默认关闭。仅当用户在 TUI/控制台 `B` 的确认模态中对显示的确切 Home 键入 `YES` 后，Guard 才会在该
Home 的 `.env` 中维护一个 BEGIN/END 标记的 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` 块。
该块影响之后从同一 Home 启动的所有 Codex 客户端，不只本次 Desktop；确认文案必须说明
这一作用域。块的编辑规则：

- 只操作精确匹配的当前版本标记块；重复块、缺损块、未知版本、非 UTF-8、超过 1 MiB
  的文件拒绝自动编辑；
- 块外字节逐字保留（含 CRLF）；通过隔离文件事务提交内容，文件内容从不回显；
- 块外已有代理键（含大小写变体、`export KEY=...` dotenv 写法与 `ALL_PROXY`）按名报告
  冲突（`BACKEND_PROXY_CONFIG_CONFLICT`），绝不抢占或追加覆盖；
- 写入在本地 NTFS 文件事务中重验原内容并原子提交，隔离外部写入及 rename-save；
  不支持事务的文件系统或运行时失败关闭，不回退非事务覆盖；
- 撤销只删除 Guard 自己未被外部修改的块；仅当文件除该块外无实质内容时才删除文件。
  撤销顺序是先安全移除块、成功后才落盘关闭授权——revoke 失败时授权、绑定 Home 与
  文件原样保留（可重试），绝不会留下 Guard 无法再定位的孤儿块；若块已移除而配置
  保存失败，仍启用的授权会在下次刷新时如实报告 `pending`；
- 未授权（包括仅展示、取消、刷新配置）时绝不创建、修改或删除 `.env`；
- Guard 绝不从自己的 `CODEX_HOME` 推断 Desktop 的 Home；自定义 Home 未确认时返回
  `BACKEND_PROXY_SCOPE_UNCONFIRMED`，不写任何文件；
- 准备成功只是文件事实（回执 `backend_proxy_config=prepared`），不是网络验证；
  Guard 不检查后端是否读取该文件，也不扩大代理交付手段。

禁止：

- 在普通启动中隐式执行任何 daemon 命令；
- `daemon start`、`daemon restart`、`daemon update`、`daemon bootstrap` 或任何形式的
  remote-control；
- 连接 app-server socket、JSON-RPC、私有 IPC、daemon PID file 或 daemon settings；
- 读取 daemon 私有状态或任何认证数据；
- 直接终止共享 Codex 进程或 Desktop（`TerminateProcess` / `taskkill` / PID 强杀 /
  Job Object）；
- 修改 `~/.codex/config.toml`，或在授权块之外写入 `~/.codex/.env`
  （包括 `features.daemon_auto_start` 之类对抗 daemon 架构的开关）。

## 权限边界

Guard 不以管理员身份启动 Desktop。Guard 用 `TokenElevation` 查询及 RAII handle；
发现自身提升运行时直接阻止 Launch（`GUARD_ELEVATED`），查询本身失败也
阻止（`GUARD_ELEVATION_QUERY_FAILED`），绝不把查询失败解释成非提升，也不自动 UAC、不创建
低权限 token、不做降权伪装。

## 信任边界

配置文件由当前用户控制，但仍必须通过 loopback HTTP 校验；host 的校验与 URL 构造使用
同一规则。APPX 与 override 路径在启动前必须是现存普通文件；APPX 查询使用原生 AppModel API（`FindPackagesByPackageFamily` 等），不启动 PowerShell。APPX 清单入口必须是安装目录内的相对路径，
canonicalize 后不得逃逸至安装目录外；清单缺失或多入口歧义时阻断，不以文件名猜测
后备程序。
`cli_executable_override` 只在指向现存绝对路径 `.exe` 时生效，失效报错而不换源；CLI 与
Desktop 的身份比较采用一致的 Windows 路径规范化（普通与 extended-length 写法等价），
名称相似但路径不可读的候选报告 Unknown 而不是“未运行”。Desktop 进程使用精确映像路径
及父子关系识别；同路径父进程存在时排除其 helper，孤立同路径进程保守视为 running。
不读取其他进程的 PEB 或命令行内存。

## 不做网络判断

Guard 不访问配置的代理端口，也不访问 OpenAI 域名。代理失效时 Desktop 仍会启动，
随后由应用自身报告网络错误。这是有意的职责边界，不是健康检查遗漏。

`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` 仅传递给新启动的进程树。它们不构成 VPN、
透明代理或防泄漏控制：Guard 不接管 DNS、UDP、系统服务或应用后续以其他路径建立的连接。
尤其不应把 ChatGPT Voice 等可能使用 UDP 的流量视为已经被 HTTP 代理覆盖。
