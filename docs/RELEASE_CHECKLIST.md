# Release Checklist

## 自动验证

- [ ] `codegraph status .`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --all-targets --locked`
- [ ] `cargo audit`
- [ ] `scripts\build-portable.cmd`
- [ ] Portable `--version` 与 `launch --help` smoke 通过。
- [ ] 验证 EXE、SHA-256 与 `build-info.json` 一致。
- [ ] `Cargo.toml` 的 repository 与 GitHub 仓库地址一致。

## 功能与安全门禁

- [ ] 仅接受 loopback HTTP/Mixed 代理；远程地址和 SOCKS-only 端口被拒绝；host 校验与 URL 构造一致。
- [ ] TUI 中按 `C` 可修改 host/port；无效配置进入修复界面而不直接退出，且保存有效配置前 Launch 持续被阻断（关闭错误提示不放行）。
- [ ] 大小写代理环境变量被正确注入，`ALL_PROXY` 被移除。
- [ ] 当前 ChatGPT Desktop、ChatGPT Classic 与显式 override 发现均可用；两者并存时优先当前应用。
- [ ] APPX 清单入口被优先使用，且解析路径不能逃逸安装目录；APPX 查询使用系统目录 PowerShell 绝对路径。
- [ ] TUI 与 `launch --json` 均显示产品类型、包名、版本、架构、发现来源与 `daemon_preparation`（skipped/stopped/not_needed），不输出安装路径或认证信息。
- [ ] 普通启动：不解析 Codex CLI、不执行任何 daemon 命令；无 CLI 的机器可正常启动。
- [ ] 修复启动：TUI `D`+`Y` 或 `launch --refresh-codex-daemon` 单次授权；取消确认（Esc/N）严格零调用；授权不持久化。
- [ ] 修复路径错误均阻断且可行动（CLI 不可用 / override 无效 / Home 无效 / 命令不支持 / stop 失败或超时），不静默降级为普通启动。
- [ ] 修复 stop 输出协议：超限/非法 UTF-8/多 JSON/未知状态一律失败；`stopped`/`notRunning` 才继续；总预算可注入测试。
- [ ] 取消契约：已取消不再 spawn；spawn 后不因取消杀 Desktop；CLI helper 超时/取消被终止并有界回收；迟到结果不写 UI。
- [ ] 管理员运行的 Guard 被阻止 Launch（`ELEVATED_LAUNCH_UNSUPPORTED`），权限查询失败同样阻止（`ELEVATION_QUERY_FAILED`），不自动 UAC。
- [ ] 等价路径（普通/`\\?\`/UNC）不漏检已运行 Desktop；名称相同但路径不可读的候选报告 Unknown 而非 Stopped。
- [ ] Guard 不执行 daemon start/restart/update/bootstrap，不连接 app-server 私有 IPC，不直接终止共享 Codex 进程，不写 `~/.codex` 配置。
- [ ] Guard 退出不终止 Desktop。
- [ ] 不存在网络探测、Usage、app-server、诊断持久化、进程终止或全流量代理入口。
- [ ] README、架构、安全与排障文档与实现一致。

## Windows 手工验收

- [ ] Windows 10 与 Windows 11 各完成一次 portable 双击启动。
- [ ] Microsoft Store 当前 ChatGPT Desktop、ChatGPT Classic（如安装）与带空格路径的 override 均可启动。
- [ ] 默认无副作用：保持一个现有共享服务/客户端运行，执行普通启动——Guard 不调用 stop、现有任务不受影响；无 CLI 的机器普通启动仍完成环境注入与 Desktop 创建。
- [ ] 代理端口从 10808 改为 7890 后：普通启动按 7890 注入（回执 skipped）；在维护窗口显式修复启动按 7890 注入（回执 stopped/not_needed）。记录 Desktop 实际连接行为与 `.env` 是否含代理键，不宣称“新 daemon 已继承”。
- [ ] 显式修复仅在维护窗口测试：先用已验证 CLI 执行 `codex app-server daemon version`，再 D+Y，观察返回状态与 Desktop 启动；同时记录 Desktop/CLI 实际包版本与版本来源，拓扑无法确认时标记“未确认”。
- [ ] “以管理员身份运行” Guard 后按 Launch：不启动 Desktop 并显示 `ELEVATED_LAUNCH_UNSUPPORTED`；正常双击启动后可正常 Launch。
- [ ] 代理未运行时，Guard 仍只执行环境注入与 Desktop 启动。
- [ ] 通过代理手工验证登录、Chat 流式输出、Work、Codex、文件上传和内置浏览器；失败时记录应用错误，Guard 不新增探测。
- [ ] 在代理或安全网关环境中确认 HTTPS 与 WebSocket Upgrade 可用；ChatGPT Voice 不作为 HTTP 代理覆盖保证。
- [ ] 如实记录 Authenticode 状态。

## GitHub Release

- [ ] 使用当前 Cargo 版本创建 `v<version>` 标签和 GitHub Release。
- [ ] Release 附件包含 `codex-proxy-guard-windows-x86_64.exe`、对应 `.sha256` 和 `build-info.json`。
- [ ] 发布页可下载，SHA-256 与 `build-info.json` 中的值一致，并标记预发布版本（如版本含 `rc`）。
