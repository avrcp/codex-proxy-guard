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

- 该次调用已获得用户明确授权（CLI `--refresh-codex-daemon` 或 TUI `D`+`Y`），授权不
  持久化到配置；
- 通过可信来源解析出的官方 Codex CLI 执行：显式 override（现存绝对路径 native `.exe`，
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

默认关闭。仅当用户在 TUI `B` 确认页对显示的确切 Home 按下 `Y` 后，Guard 才会在该
Home 的 `.env` 中维护一个 BEGIN/END 标记的 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` 块。
该块影响之后从同一 Home 启动的所有 Codex 客户端，不只本次 Desktop；确认文案必须说明
这一作用域。块的编辑规则：

- 只操作精确匹配的当前版本标记块；重复块、缺损块、未知版本、非 UTF-8、超过 1 MiB
  的文件拒绝自动编辑；
- 块外字节逐字保留（含 CRLF），文件从不被整体重写，文件内容从不回显；
- 块外已有代理键（含大小写变体与 `ALL_PROXY`）按名报告冲突（`BACKEND_PROXY_CONFIG_CONFLICT`），
  绝不抢占或追加覆盖；
- 写入经同目录临时文件原子替换，替换前复核原内容未变；
- 撤销只删除 Guard 自己未被外部修改的块；仅当文件除该块外无实质内容时才删除文件；
- 未授权（包括仅展示、取消、刷新配置）时绝不创建、修改或删除 `.env`；
- Guard 绝不从自己的 `CODEX_HOME` 推断 Desktop 的 Home；自定义 Home 未确认时返回
  `BACKEND_PROXY_SCOPE_UNCONFIRMED`，不写任何文件；
- 准备成功只是文件事实（`backend_proxy_config_prepared`），不是网络验证；实际内置
  后端不读取该文件时记录 `BACKEND_PROXY_DELIVERY_UNSUPPORTED`，不扩大手段。

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

Guard 不以管理员身份启动 Desktop：Codex 0.157+ 的共享后台服务要求非提升进程。Guard 用
与 Codex 官方一致的 `TokenElevation` 查询（RAII handle、失败即时捕获 last_os_error）；
发现自身提升运行时直接阻止 Launch（`ELEVATED_LAUNCH_UNSUPPORTED`），查询本身失败也
阻止（`ELEVATION_QUERY_FAILED`），绝不把查询失败解释成非提升，也不自动 UAC、不创建
低权限 token、不做降权伪装。

## 信任边界

配置文件由当前用户控制，但仍必须通过 loopback HTTP 校验；host 的校验与 URL 构造使用
同一规则。APPX 与 override 路径在启动前必须是现存普通文件；APPX 查询固定使用系统目录
下 Windows PowerShell 的绝对路径。APPX 清单入口必须是安装目录内的相对路径，
canonicalize 后不得逃逸至安装目录外；清单缺失或多入口歧义时阻断，不以文件名猜测
后备程序。
`cli_executable_override` 只在指向现存绝对路径 `.exe` 时生效，失效报错而不换源；CLI 与
Desktop 的身份比较采用一致的 Windows 路径规范化（普通与 extended-length 写法等价），
名称相似但路径不可读的候选报告 Unknown 而不是“未运行”。Desktop “已运行”只由与已发现
可执行文件路径相同、具有有效启动时间且不是 Chromium `--type=` 子进程的根进程证明。

## 不做网络判断

Guard 不访问配置的代理端口，也不访问 OpenAI 域名。代理失效时 Desktop 仍会启动，
随后由应用自身报告网络错误。这是有意的职责边界，不是健康检查遗漏。

`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` 仅传递给新启动的进程树。它们不构成 VPN、
透明代理或防泄漏控制：Guard 不接管 DNS、UDP、系统服务或应用后续以其他路径建立的连接。
尤其不应把 ChatGPT Voice 等可能使用 UDP 的流量视为已经被 HTTP 代理覆盖。

