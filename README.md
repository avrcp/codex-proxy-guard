# Codex Proxy Guard

Windows 控制台本地代理启动器：纯 C++20 + Win32，无 Qt、无 PowerShell discovery。`0.7.0-rc.4` 是一个 Console 子系统单文件 EXE（静态 CRT `/MT`；全部对象以 `/guard:cf` 编译，最终映像链接 ASLR/DEP/高熵 VA/CET 加固——CET 以扩展 DLL 特征记录为准），默认提供原生 VT TUI，并通过 Windows 应用模型正确激活已注册的 ChatGPT Desktop。

Guard 将代理参数交给新启动的 ChatGPT Desktop / Codex Desktop，并可在明确授权的 Codex Home 中维护一个代理环境块。它不测量网络连通性，不管理代理服务，不读取认证数据，也不修改 Windows 系统代理或 Codex `config.toml`。

## 使用（单文件）

正式产物是 `dist/releases/<version>-<commit>/CodexProxyGuard.exe`：复制这一个文件即可运行。无 DLL 依赖、无需安装 VC redist；重命名或放进含中文/空格的目录均正常工作。实际大小与 SHA-256 记录在每次构建的 manifest 中；导入表只依赖 Windows 系统 DLL。

TUI 不是 daemon：关闭 Guard 不会关闭 Desktop；Guard 不管理代理软件，不检测网络，不修改系统代理。VT 输出不可用时自动回退行式控制台。

所有角色共用同一个 EXE：

```powershell
.\CodexProxyGuard.exe                      # VT TUI（默认）
.\CodexProxyGuard.exe console              # 同上；--plain 强制行式控制台
.\CodexProxyGuard.exe config-path
.\CodexProxyGuard.exe init-config --proxy-host 127.0.0.1 --proxy-port 10808
.\CodexProxyGuard.exe launch --json
.\CodexProxyGuard.exe launch --refresh-codex-daemon --json
.\CodexProxyGuard.exe launch --activation-only --json
.\CodexProxyGuard.exe build-info           # 输出当前版本、commit、dirty 状态
.\CodexProxyGuard.exe licenses             # 离线输出全部内嵌许可与 notices
```

TUI 按键：`Enter`/`L` 启动、`E` 编辑代理、`B` 授权/撤销 Codex 后端代理块、`D` 修复启动、`R`/`F5` 刷新、`?`/`F1` 帮助、`Ctrl+C` 取消前台操作、`Q` 退出。授权与修复须在模态中键入 `YES`；编辑器支持 Tab 切换、Home/End/方向键/退格/删除。TUI 和 plain 通过同进程引擎会话提交操作；原生激活使用短命的自有 worker 子进程。忙时 Q 需明确选择取消并退出，等待自有操作回收后退出；空闲 Ctrl+C 不修改授权提案。

代理编辑器在 Host 或 Port 上按 Enter 都提交完整表单；保存失败保留草稿。若已提交后刷新失败，只重试读取。授权弹窗绑定打开时显示的确切 Home 和一次性提案，配置变化后须重新确认。

启动成功后区分实例观测：`created` 表示观察到新进程；`reused` 表示复用了已运行的 Desktop 实例——代理设置是否重新生效未确认，请完全退出 Desktop 后再从 Guard 启动；`unknown` 表示无法确认。这些是引擎观测分类，不是网络验证。

修复启动每次单独确认，可执行一次公开的 `codex app-server daemon stop`，可能中断其他客户端共享任务。注册应用必须先授权并成功准备代理块。提交激活后取消可能只能报告结果未知（`APPX_ACTIVATION_OUTCOME_UNKNOWN`），不会自动重试。

许可与第三方声明（项目 MIT、toml++、nlohmann/json）全部内嵌在 EXE 内：`licenses` 子命令输出同一内容，离线可用。

## 配置

`--config` 缺省时所有交互入口使用同一个默认路径；相对路径在启动时固定为绝对路径。显式空值、重复参数和含非 ASCII 数字的端口均拒绝。`launch --json` 输出一份 JSON，普通 launch 输出人类摘要，错误走 stderr。

默认文件位于 `%APPDATA%\codex-proxy-guard\config.toml`，保留版本 2 的 TOML 数据结构与已有 Home 授权绑定。未知字段、错误类型和无效配置会阻止启动；TUI/控制台可修复代理配置。

```toml
version = 2
[proxy]
scheme = "http"
host = "127.0.0.1"
port = 10808
no_proxy = ["localhost", "127.0.0.1", "::1"]
[codex]
refuse_if_running = true
manage_codex_proxy_env = false
[tui]
alternate_screen = "auto"
```

`executable_override` 和 `cli_executable_override` 是可选的绝对 EXE 路径；注册包的 EXE 覆盖仍须经应用模型激活。`tui.alternate_screen` 控制 TUI 备用屏幕（`auto`/`always`/`never`；`always` 且 VT 不可用时回退行式控制台）。

Guard 的块由 `# BEGIN CODEX PROXY GUARD: proxy-v1` 和对应 END 标记界定。块外原字节保留；现有代理变量（含 `export` 拼写）、异常标记或被修改的块会阻止写入。撤销先安全移除块，再保存关闭授权；失败保持授权和绑定 Home。`init-config --force` 不能覆盖尚未撤销的授权。

自动编辑有受限的语法边界：跨物理行的引号值、续行或未闭合引号会被整体拒绝（`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED`），不修改任何字节，授权保持可重试。该限制防止把引号字符串内的标记误认成 Guard 的托管块。

## 开发与打包

要求 Windows、MSVC x64、CMake ≥ 3.25、Ninja、Python 3。生产依赖仅 Windows SDK/STL 与 vendored toml++/nlohmann/json；测试使用 vendored doctest。

```powershell
.\scripts\test-noqt.ps1                    # 开发构建 + 全部测试
.\scripts\build-portable.cmd -AllowUnsignedDev   # 未签名开发发布
.\scripts\build-portable.cmd -Sign -CertificateSha1 <thumbprint>   # 正式签名发布
```

发布流水线（clean tree → 固定 commit 的 git archive → 独立源码/构建目录 → 构建 → 全部测试 → 打包 EXE smoke → 导入表扫描 → 可选 Authenticode 签名与验签 → SHA-256 → transport ZIP + source ZIP + size report → release-manifest.json）由 `scripts/verify-package.py --schema static-single` 独立重验后提升到 `dist/releases/<version>-<commit>/`。未签名产物处处标注 `UNSIGNED DEVELOPMENT BUILD`；公开分发要求有效 Authenticode。

详见[架构](docs/ARCHITECTURE.md)、[安全边界](docs/SECURITY.md)、[TUI 验收](docs/TUI_ACCEPTANCE.md)、[发布就绪状态](docs/RELEASE_READINESS.md)、[迁移记录](docs/CPP_TUI_MIGRATION_PLAN.md)与[第三方许可](THIRD_PARTY_NOTICES.md)。

`.env` 写入与撤销要求本地 NTFS 事务可用；不支持时保留授权并拒绝修改，无非事务回退。
