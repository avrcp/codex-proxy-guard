# Codex Proxy Guard

Windows 控制台本地代理启动器：纯 Rust 2024 + Win32（`windows` crate）+ Ratatui TUI，无 Qt、无 PowerShell discovery、无 C++ 桥接。`0.8.0-rc.1` 是一个 Console 子系统单文件 EXE（静态 CRT；每个 crate 以 `-C control-flow-guard=checks` 编译，最终映像链接 `/guard:cf /CETCOMPAT /DYNAMICBASE /NXCOMPAT /HIGHENTROPYVA` 加固——CET 以扩展 DLL 特征记录为准），默认提供 Ratatui VT TUI，并通过 Windows 应用模型正确激活已注册的 ChatGPT Desktop。

Guard 将代理参数交给新启动的 ChatGPT Desktop / Codex Desktop，并可在明确授权的 Codex Home 中维护一个代理环境块。它不测量网络连通性，不管理代理服务，不读取认证数据，也不修改 Windows 系统代理或 Codex `config.toml`。

## 使用（单文件）

正式产物是 `dist/releases/<version>-<commit>/CodexProxyGuard.exe`：复制这一个文件即可运行。无 DLL 依赖、无需安装 VC redist；重命名或放进含中文/空格的目录均正常工作。SHA-256 记录在发布目录的 `SHA256SUMS` 中；导入表只依赖 Windows 系统 DLL。

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

TUI 按键：`Enter`/`L` 启动、`E` 编辑代理、`B` 授权/撤销 Codex 后端代理块、`D` 修复启动、`R`/`F5` 刷新、`?`/`F1` 帮助、`Ctrl+C` 取消前台操作、`Q` 退出。授权与修复须在模态中键入 `YES`；编辑器支持 Tab / Shift+Tab 切换与字符级光标编辑（`←`/`→`/`Home`/`End`/`Backspace`/`Delete`，端口仅数字；带 Ctrl/Alt 的组合键一律不当作字母输入）。统一忽略按键松开事件，避免字符重复和焦点来回跳转，并保留长按连续输入、移动和删除。TUI 和 plain 通过同进程引擎会话提交操作；原生激活使用短命的自有 worker 子进程。忙时 Q 需明确选择取消并退出，等待自有操作回收后退出；空闲 Ctrl+C 不修改授权提案。

代理编辑器在 Host 或 Port 上按 Enter 都提交完整表单；保存失败保留草稿。若已提交后刷新失败，只重试读取。授权弹窗绑定打开时显示的确切 Home 和一次性提案，配置变化后须重新确认。

主界面呈现的是引擎事实，不是界面推断：`PROXY` 行显示最近一次成功读取（或刚刚提交）的配置端点，读取失败时保留该端点并标注 `last good read`/`saved, not re-read`，不会把显示的端口退回内置默认值；`PLAN` 行说明这个端点将经哪个通道投递——激活参数、进程环境、未确认（按 `R` 刷新）或无法构造（给出拒绝错误码）。注册应用只有在能构造出激活参数时才允许启动，判定由引擎给出，界面不重复计算。

启动成功后区分实例观测：`created` 表示观察到新进程；`reused` 表示复用了已运行的 Desktop 实例——代理设置是否重新生效未确认，请完全退出 Desktop 后再从 Guard 启动；`unknown` 表示无法确认。这些是引擎观测分类，不是网络验证。结果面板同时列出 `Configured`（当前配置）与 `Submitted`（本次实际提交的端点）以及投递通道；两者不一致会明确提示，Guard 只确认投递事实，不确认 Desktop 已采用，也不做网络验证。

修复启动每次单独确认，可执行一次公开的 `codex app-server daemon stop`，可能中断其他客户端共享任务。注册应用必须先授权并成功准备代理块。提交激活后取消可能只能报告结果未知（`APPX_ACTIVATION_OUTCOME_UNKNOWN`），不会自动重试。

许可与第三方声明（项目 MIT；ratatui、crossterm、toml、windows-rs 的上游许可）全部内嵌在 EXE 内：`licenses` 子命令输出同一内容，离线可用。

## 配置

`--config` 缺省时所有交互入口使用同一个默认路径；相对路径在启动时固定为绝对路径。显式空值、重复参数和含非 ASCII 数字的端口均拒绝。`launch --json` 输出一份 JSON，普通 launch 输出人类摘要，错误走 stderr。

默认文件位于 `%APPDATA%\codex-proxy-guard\config.toml`，保留版本 2 的 TOML 数据结构与已有 Home 授权绑定。未知字段、错误类型和无效配置会阻止启动；TUI/控制台可修复代理配置。

`no_proxy` 允许保存任意条目（后端块原样写入），但 Chromium 的 `--proxy-bypass-list` 只支持 `localhost` 与回环地址：其余条目会让注册应用的启动参数无法构造（`PROXY_BYPASS_UNSUPPORTED`），`PLAN` 行随即显示 `not buildable`，启动在取得配置 lease 之前被拒绝；原生 EXE 目标经进程环境变量投递，不受该限制影响。

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

`executable_override` 和 `cli_executable_override` 是可选的绝对 EXE 路径；注册包的 EXE 覆盖仍须经应用模型激活。`tui.alternate_screen` 控制 TUI 备用屏幕（`auto`/`always`/`never`；VT 不可用时回退行式控制台）。

Guard 的块由 `# BEGIN CODEX PROXY GUARD: proxy-v1` 和对应 END 标记界定。块外原字节保留；现有代理变量（含 `export` 拼写）、异常标记或被修改的块会阻止写入。撤销先安全移除块，再保存关闭授权；失败保持授权和绑定 Home。`init-config --force` 不能覆盖尚未撤销的授权。

自动编辑有受限的语法边界：跨物理行的引号值、续行或未闭合引号会被整体拒绝（`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED`），不修改任何字节，授权保持可重试。该限制防止把引号字符串内的标记误认成 Guard 的托管块。

## 开发与打包

要求 Windows、Rust stable（`rust-toolchain.toml` 固定 1.96.0，`x86_64-pc-windows-msvc`）、MSVC 链接工具。生产依赖仅 `windows`/`windows` COM 绑定、ratatui/crossterm、toml；一切 Win32 调用集中在 `crates/cpg-windows`。

```powershell
cargo test --workspace                     # 99 个测试（另有 3 个 ConPTY TUI 验收测试暂以 #[ignore] 保留，见迁移验收文档）
.\scripts\test-rust.ps1                    # fmt + clippy(-D warnings) + tests + diff 检查
.\scripts\build-portable-rust.cmd -AllowUnsignedDev       # 未签名开发发布
.\scripts\build-portable-rust.cmd -Sign -CertificateSha1 <thumbprint>   # 正式签名发布
```

发布流水线（clean tree → 固定 commit 的 git archive → 独立源码目录 → `cargo build --release --locked`（完整硬化 RUSTFLAGS）→ release 模式全测试 → 打包 EXE smoke → SHA-256 → source.zip → 不可变提升到 `dist/releases/<version>-rust-<commit>/`）。未签名产物处处标注 `UNSIGNED DEVELOPMENT BUILD`；公开分发要求有效 Authenticode。PE 缓解与导入表以 `scripts/verify-package.py` 的解析核心核验。

详见[架构](docs/ARCHITECTURE.md)、[安全边界](docs/SECURITY.md)、[迁移验收](docs/RUST_TUI_MIGRATION_ACCEPTANCE.md)、[端口链路验收](docs/PROXY_PORT_CHAIN_ACCEPTANCE.md)、[发布就绪状态](docs/RELEASE_READINESS.md)、[基线契约](docs/RUST_MIGRATION_BASELINE.md)与[第三方许可](THIRD_PARTY_NOTICES.md)。

`.env` 写入与撤销要求本地 NTFS 事务可用；不支持时保留授权并拒绝修改，无非事务回退。

## 历史实现

C++（0.7.x）与 Qt（0.6.x）实现已从工作树退役，可经 git 历史检索（C++ 最后提交 `0bdf2ed`；Qt 时代见 `v0.6.1-rc.1` 标签）。
