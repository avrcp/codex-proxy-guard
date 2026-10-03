# Codex Proxy Guard

Windows 上的本地 HTTP/Mixed 代理启动器。`0.6.1-rc.1` 全部采用 C++20、Qt 6.8.3 和 Windows SDK；一个静态链接的生产 EXE 同时承担 GUI、引擎桥、激活 worker 和 CLI 角色，不再包含 Rust 运行时或构建依赖。

Guard 将代理参数交给新启动的 ChatGPT Desktop / Codex Desktop，并可在明确授权的 Codex Home 中维护一个代理环境块。它不测量网络连通性，不管理代理服务，不读取认证数据，也不修改 Windows 系统代理或 Codex `config.toml`。

## 使用（单文件）

正式产物是 `dist/releases/<version>-<commit>/CodexProxyGuard.exe`：复制这一个文件即可运行，不需要同目录 Qt DLL、CRT DLL、`platforms/` 插件目录或 `engine/` 子目录，也不需要安装 VC redist、修改 PATH 或安装 Qt。EXE 静态链接 Qt 6.8.3 与静态 CRT（/MT），导入表只依赖 Windows 系统 DLL。重命名 EXE 或放进含中文/空格的目录均正常工作。

GUI 与引擎仍是两个进程：GUI 进程通过 `QCoreApplication::applicationFilePath()` 以 `bridge` 参数重新启动自身作为引擎子进程，激活 worker 同样由自身路径派生——一个文件、多个进程角色，进程隔离边界与授权模型不变。注册应用通过 Windows 原生应用模型激活，普通独立 EXE 使用进程环境。

GUI 可修改 loopback 地址与端口、刷新本地状态、授权或撤销后台代理块，以及启动 Desktop。代理必须为本机 HTTP/Mixed 端口；不支持 SOCKS-only、远程代理或带凭据的 URL。代理编辑窗口在保存被引擎确认前保持打开：校验、锁定或写入失败时输入原样保留，可原位修正后重试；保存过程中引擎中断则如实报告结果未确认，不会自动重发。

授权对话框显示确切 Home，默认取消。后台代理块影响后来使用这个 Home 的 Codex 进程；“已准备”仅表示文件状态，不能证明网络已通过代理。普通启动绝不解析 Codex CLI，也不调用 daemon 命令。

启动成功后 GUI 会区分实例观测：`created` 表示观察到新进程；`reused` 表示复用了已运行的 Desktop 实例——本次启动的代理设置是否重新生效未确认，请完全退出 Desktop 后再从 Guard 启动；`unknown` 表示无法确认实例是否新建。这些是引擎观测分类，不是网络验证。

修复启动每次单独确认，可执行一次公开的 `codex app-server daemon stop`，可能中断其他客户端共享任务。注册应用必须先授权并成功准备代理块。关闭 Guard 不会终止 Desktop；提交激活后取消可能只能报告结果未知，不能自动重试。即使引擎已离线或已崩溃，关闭窗口仍会正常完成，无需任务管理器。

## CLI 与控制台

无参数启动进入 GUI。`--config PATH`（无子命令）以指定配置启动 GUI，并把同一绝对路径传给自身桥进程；`--smoke-test` 构造并关闭窗口（测试用）。所有 headless 角色共用同一个 EXE：

```powershell
.\CodexProxyGuard.exe
.\CodexProxyGuard.exe config-path
.\CodexProxyGuard.exe init-config --proxy-host 127.0.0.1 --proxy-port 10808
.\CodexProxyGuard.exe launch --json
.\CodexProxyGuard.exe launch --refresh-codex-daemon --json
.\CodexProxyGuard.exe launch --activation-only --json
.\CodexProxyGuard.exe build-info        # --build-info 拼写等价
.\CodexProxyGuard.exe licenses          # 离线输出全部内嵌许可与 notices
.\CodexProxyGuard.exe console           # 行式控制台
```

`console` 进入行式控制台：`L` 启动、`E` 编辑、`B` 授权/撤销、`R` 修复、`S` 刷新、`Q` 退出；授权和修复须键入 `YES`。GUI 是主要交互入口。`--config PATH` 可为所有命令指定 Guard 配置（激活 worker 不接受配置参数）。激活诊断不写代理块、不传代理参数，也不能与修复组合。参数在构造任何 Application 对象之前结构化分类：`--config` 的值即使含有 `bridge` 等字样也不会被误判为角色。

## 配置

默认文件位于 `%APPDATA%\codex-proxy-guard\config.toml`——单文件发布不改变配置位置、授权绑定或启动锁，没有“配置跟 EXE 走”的 portable 模式。保留版本 2 的 TOML 数据结构与已有 Home 授权绑定，避免遗留无法撤销的代理块。未知字段、错误类型和无效配置会阻止启动；GUI/控制台可修复代理配置。

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

`executable_override` 和 `cli_executable_override` 是可选的绝对 EXE 路径；注册包的 EXE 覆盖仍须经应用模型激活。`proxy_env_home` 只与明确授权的后台代理块配对。保留 `tui.alternate_screen` 供读取既有配置，它不再控制控制台布局。不要手工设置授权字段代替确认流程。

Guard 的块由 `# BEGIN CODEX PROXY GUARD: proxy-v1` 和对应 END 标记界定。块外原字节保留；现有代理变量（含 `export` 拼写）、异常标记或被修改的块会阻止写入。撤销先安全移除块，再保存关闭授权；失败保持授权和绑定 Home。`init-config --force` 不能覆盖尚未撤销的授权。

自动编辑有受限的语法边界：`.env` 中的单行赋值、单行单/双引号值（含转义）、注释、空行和 `export` 拼写可被识别；跨物理行的引号值、续行或未闭合引号会被整体拒绝（`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED`），不修改任何字节，授权保持可重试。“不支持自动编辑”不等于该文件无效——请手工修整后重试。此限制防止把引号字符串内的标记误认成 Guard 的托管块。

## 开发与打包

要求 Windows、MSVC x64、CMake ≥ 3.25、Ninja、Python 3（py7zr）。动态开发构建使用 Qt 6.8.3 Core/Gui/Widgets/Test SDK；正式单文件发布额外使用由本仓库固定配方构建的静态 Qt SDK（含静态 CRT）。

```powershell
.\scripts\test-cpp.ps1 -QtRoot C:\Qt\6.8.3\msvc2022_64   # 动态开发构建 + 全部测试
.\scripts\build-qt-static.ps1                             # 一次性构建静态 Qt SDK（缓存于 target/qt-static）
.\scripts\build-portable.cmd -StaticQt                    # 正式单文件发布（clean tree，promote 到 dist/releases）
.\scripts\build-portable.cmd                              # 过渡期动态拆分包（dist/gui + dynamic ZIP）
```

正式 profile 要求 clean 工作树，staging 成功后才提升到 `dist/releases/<version>-<commit>/`：单个 `CodexProxyGuard.exe`、仅含该 EXE 的 transport ZIP、`source-compliance` 配套包（应用源码快照、上游 Qt 源码归档、静态配方、许可与重建说明）、`release-manifest.json`、`SHA256SUMS.txt` 与体积报告。发布前由 `verify-package.py --schema static-single` 独立重开全部产物校验，包括执行解压后的 EXE 读取内嵌 build-info 与导入表扫描证据。静态插件集合显式固定（Windows QPA、gif/ico/jpeg、modern style），测试二进制单独链接 offscreen。

替换/重建 Qt：静态构建下需按配套包内 `recipes/qt-static-recipe.json` 重建静态 SDK 并重链接应用；详见 [静态 Qt 重建](docs/STATIC_QT_REBUILD.md)。用户自建 Qt 不受官方 hash 门禁限制。

详见[架构](docs/ARCHITECTURE.md)、[安全边界](docs/SECURITY.md)、[发布门禁](docs/RELEASE_CHECKLIST.md)、[单文件验收](docs/SINGLE_EXE_ACCEPTANCE.md) 和 [第三方许可](THIRD_PARTY_NOTICES.md)。

`.env` 写入与撤销要求本地 NTFS 事务可用；不支持时保留授权并拒绝修改，无非事务回退。
该严格并发保护与其 Windows API 生命周期取舍记录在[架构决策](docs/ARCHITECTURE.md#filesystem-transaction-decision)。
