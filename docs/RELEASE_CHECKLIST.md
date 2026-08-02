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

- [ ] 仅接受 loopback HTTP/Mixed 代理；远程地址和 SOCKS-only 端口被拒绝。
- [ ] TUI 中按 `C` 可修改 host/port；无效配置进入修复界面而不直接退出。
- [ ] 大小写代理环境变量被正确注入，`ALL_PROXY` 被移除。
- [ ] 当前 ChatGPT Desktop、ChatGPT Classic 与显式 override 发现均可用；两者并存时优先当前应用。
- [ ] APPX 清单入口被优先使用，且解析路径不能逃逸安装目录；仅在入口缺失时使用受控后备路径。
- [ ] TUI 与 `launch --json` 均显示产品类型、包名、版本、架构和发现来源，不输出安装路径或认证信息。
- [ ] 已运行的 Desktop 阻止再次启动；并发 Guard 启动锁有效。
- [ ] Guard 退出不终止 Desktop。
- [ ] 不存在网络探测、Usage、app-server、诊断持久化、进程终止或全流量代理入口。
- [ ] README、架构、安全与排障文档与实现一致。

## Windows 手工验收

- [ ] Windows 10 与 Windows 11 各完成一次 portable 双击启动。
- [ ] Microsoft Store 当前 ChatGPT Desktop、ChatGPT Classic（如安装）与带空格路径的 override 均可启动。
- [ ] 代理未运行时，Guard 仍只执行环境注入与 Desktop 启动。
- [ ] 通过代理手工验证登录、Chat 流式输出、Work、Codex、文件上传和内置浏览器；失败时记录应用错误，Guard 不新增探测。
- [ ] 在代理或安全网关环境中确认 HTTPS 与 WebSocket Upgrade 可用；ChatGPT Voice 不作为 HTTP 代理覆盖保证。
- [ ] 如实记录 Authenticode 状态。

## GitHub Release

- [ ] 使用当前 Cargo 版本创建 `v<version>` 标签和 GitHub Release。
- [ ] Release 附件包含 `codex-proxy-guard-windows-x86_64.exe`、对应 `.sha256` 和 `build-info.json`。
- [ ] 发布页可下载，SHA-256 与 `build-info.json` 中的值一致，并标记预发布版本（如版本含 `rc`）。
