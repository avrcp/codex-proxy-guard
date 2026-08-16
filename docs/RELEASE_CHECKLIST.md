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
- [ ] Managed stale binding 与 `ManagedNodeState` 一致，已移除/失去地区 hint 的节点不参与选择。
- [ ] Benchmark cache 校验 fingerprint、expected region 与 TTL；损坏缓存按 miss 重算且写入可恢复。
- [ ] Managed 启动用同一个 sidecar 完成 Geo/ChatGPT HEAD 复验；失败时不启动 Desktop。
- [ ] Managed one-shot `launch` 被拒绝，TUI 退出会等待自有 sidecar 回收。
- [ ] 不存在持续网络监控、Usage、app-server、诊断持久化、Desktop 终止或全流量代理入口。

## Local Web Manager 门禁

- [ ] `M` 打开默认浏览器，仅绑定 `127.0.0.1:<ephemeral>`，无 LAN/固定端口/常驻服务。
- [ ] 每次启动生成独立 256-bit token；token 不出现在状态/订阅/节点 DTO、错误、日志或任何持久存储。
- [ ] API 无 token → 401，错误 token → 401，正确 token → 200；非 GET 校验 Origin，Host 必须为本机端口。
- [ ] 所有响应含 `no-store`/`nosniff`/`no-referrer`/`same-origin` 与 HTML CSP；资源编译进 EXE，无远程前端。
- [ ] Web 无 Desktop launch API，不拥有 managed sidecar，不返回已保存 URL 或 raw outbound。
- [ ] subscription inspect/add/edit/sync/activate/delete 可用；删除激活中的订阅返回 409。
- [ ] 节点 JP/SG/US 过滤与 healthy/rejected/not-tested/stale 状态正确；AUTO 仍为 JP > SG > US。
- [ ] benchmark 从浏览器后台执行、HTTP 快速返回、页面轮询；忙时 `409 OPERATION_BUSY` 不排队。
- [ ] 手选仅限 Fresh+Healthy 且属于激活订阅的节点，仅对当前进程会话有效；激活切换/新 benchmark 清除手选。
- [ ] Manager 打开时锁定 TUI 运行时变更；关闭后 TUI 重新加载配置与状态；Guard 退出停止 Manager。
- [ ] README、架构、安全与排障文档与实现一致。

## Windows 手工验收

- [ ] Windows 10 与 Windows 11 各完成一次 portable 双击启动。
- [ ] Microsoft Store 当前 ChatGPT Desktop、ChatGPT Classic（如安装）与带空格路径的 override 均可启动。
- [ ] 代理未运行时，Guard 仍只执行环境注入与 Desktop 启动。
- [ ] 通过代理手工验证登录、Chat 流式输出、Work、Codex、文件上传和内置浏览器；失败时记录应用错误，Guard 不新增探测。
- [ ] 在代理或安全网关环境中确认 HTTPS 与 WebSocket Upgrade 可用；ChatGPT Voice 不作为 HTTP 代理覆盖保证。
- [ ] 使用官方 sing-box 本地文件验证默认 runtime 路径与显式 `managed.sing_box_path`。
- [ ] 如实记录 Authenticode 状态。
- [ ] Windows 手工 smoke：TUI `M` → 浏览器打开 → Add Airport → Inspect → Sync → 仅 JP/SG/US 可见
      → Benchmark → Healthy JP 为 AUTO winner → 手选另一个 Healthy JP → 关闭 Manager
      → TUI 显示 MANUAL → `Enter` 启动 → 所选节点仍通过 `start_verified_sidecar` → 退出后
      Manager 与 managed sidecar 均停止。

## GitHub Release

- [ ] 使用当前 Cargo 版本创建 `v<version>` 标签和 GitHub Release。
- [ ] Release 附件包含 `codex-proxy-guard-windows-x86_64.exe`、对应 `.sha256` 和 `build-info.json`。
- [ ] 发布页可下载，SHA-256 与 `build-info.json` 中的值一致，并标记预发布版本（如版本含 `rc`）。
