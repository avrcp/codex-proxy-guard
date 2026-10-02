# Codex Proxy Guard

Windows 上的本地 HTTP/Mixed 代理启动器。`0.6.0-rc.1` 全部采用 C++20、Qt 6.8.3 和 Windows SDK；Qt GUI、控制台及 CLI 共用 C++ 引擎，不再包含 Rust 运行时或构建依赖。

Guard 将代理参数交给新启动的 ChatGPT Desktop / Codex Desktop，并可在明确授权的 Codex Home 中维护一个代理环境块。它不测量网络连通性，不管理代理服务，不读取认证数据，也不修改 Windows 系统代理或 Codex `config.toml`。

## 使用

解压 `CodexProxyGuard-0.6.0-rc.1-windows-x86_64.zip`，运行 `CodexProxyGuard.exe`。保留整个目录；无需安装 Qt 或开发工具。注册应用通过 Windows 原生应用模型激活，普通独立 EXE 使用进程环境。

GUI 可修改 loopback 地址与端口、刷新本地状态、授权或撤销后台代理块，以及启动 Desktop。代理必须为本机 HTTP/Mixed 端口；不支持 SOCKS-only、远程代理或带凭据的 URL。

授权对话框显示确切 Home，默认取消。后台代理块影响后来使用这个 Home 的 Codex 进程；“已准备”仅表示文件状态，不能证明网络已通过代理。普通启动绝不解析 Codex CLI，也不调用 daemon 命令。

修复启动每次单独确认，可执行一次公开的 `codex app-server daemon stop`，可能中断其他客户端共享任务。注册应用必须先授权并成功准备代理块。关闭 Guard 不会终止 Desktop；提交激活后取消可能只能报告结果未知，不能自动重试。

## CLI 与控制台

```powershell
.\engine\codex-proxy-guard.exe
.\engine\codex-proxy-guard.exe config-path
.\engine\codex-proxy-guard.exe init-config --proxy-host 127.0.0.1 --proxy-port 10808
.\engine\codex-proxy-guard.exe launch --json
.\engine\codex-proxy-guard.exe launch --refresh-codex-daemon --json
.\engine\codex-proxy-guard.exe launch --activation-only --json
.\engine\codex-proxy-guard.exe build-info
```

无子命令进入行式控制台：`L` 启动、`E` 编辑、`B` 授权/撤销、`R` 修复、`S` 刷新、`Q` 退出；授权和修复须键入 `YES`。原 Rust 全屏 TUI 已移除，GUI 是主要交互入口。`--config PATH` 可为所有命令指定 Guard 配置。激活诊断不写代理块、不传代理参数，也不能与修复组合。

## 配置

默认文件位于 `%APPDATA%\codex-proxy-guard\config.toml`。保留版本 2 的 TOML 数据结构与已有 Home 授权绑定，避免遗留无法撤销的代理块。未知字段、错误类型和无效配置会阻止启动；GUI/控制台可修复代理配置。

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

## 开发与打包

要求 Windows、MSVC x64、CMake ≥ 3.25、Ninja、Qt 6.8.3 Core/Gui/Widgets/Test。发布脚本另需 Python 3、py7zr，并自动使用校验过的官方 Qt SDK/源码归档。

```powershell
.\scripts\test-cpp.ps1 -QtRoot C:\Qt\6.8.3\msvc2022_64
.\scripts\build-portable.cmd
```

第一条编译 C++ 并执行全部后端和 GUI 测试。canonical portable 脚本构建两个 C++ EXE、运行测试、部署动态 Qt/VC 运行库、执行隔离 PATH 冒烟测试、核对内嵌 commit/dirty 并生成 SHA-256 清单和 ZIP。`build-gui.cmd` 是同一流程的入口别名。输出目录为 `dist/gui`，ZIP 位于 `dist`。包内含 Qt LGPL 文本、对应源码和 toml++ MIT 许可证。

详见 [迁移计划](docs/CPP_ENGINE_MIGRATION_PLAN.md)、[架构](docs/ARCHITECTURE.md)、[安全边界](docs/SECURITY.md)、[发布门禁](docs/RELEASE_CHECKLIST.md) 和 [第三方许可](THIRD_PARTY_NOTICES.md)。
