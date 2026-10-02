# Release Checklist

## 自动验证

- [ ] `scripts\build-gui.cmd`：Qt 6.8.3 dynamic、CTest、windeployqt、app-local CRT。
- [ ] GUI `--smoke-test` 无引擎/业务副作用，`--build-info` 与 engine/包清单一致。
- [ ] GUI ZIP SHA、逐文件 SHA、Qt 对应源码 SHA 与许可证完整。
- [ ] GUI 协议和 controller fake-engine 测试覆盖失联、确认、取消、未知状态。
- [ ] [GUI 验收记录](CPP_GUI_ACCEPTANCE.md) 如实区分自动 smoke 与真实环境验收。

- [ ] `codegraph status .`
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `cargo test --workspace --all-targets --locked`
- [ ] `cargo audit`
- [ ] `scripts\build-portable.cmd`
- [ ] Portable `--version`、`launch --help`、`build-info` 与激活 worker 拒绝畸形输入 smoke 通过。
- [ ] 验证 EXE、SHA-256 与 `build-info.json` 一致；`build-info` 的 commit/dirty 与构建源一致。
- [ ] `Cargo.toml` 的 repository 与 GitHub 仓库地址一致。

## 功能与安全门禁

- [ ] 仅接受 loopback HTTP/Mixed 代理；远程地址和 SOCKS-only 端口被拒绝；host 校验与 URL 构造一致。
- [ ] TUI 中按 `C` 可修改 host/port；无效配置进入修复界面而不直接退出，且保存有效配置前 Launch 持续被阻断（关闭错误提示不放行）。
- [ ] 大小写代理环境变量被正确注入，`ALL_PROXY` 被移除。
- [ ] 当前 ChatGPT Desktop、ChatGPT Classic 与显式 override 发现均可用；两者并存时优先当前应用。
- [ ] APPX 清单入口被优先使用，且解析路径不能逃逸安装目录；APPX 查询使用系统目录 PowerShell 绝对路径。
- [ ] FullName、FamilyName、Application.Id、AUMID 来自同一注册快照；多 Application 不任选首个，包内 override 不降级成普通 EXE。
- [ ] discovery payload 使用 schema_version=1 固定 envelope（resources/appx-discovery.ps1 与生产同源）。
- [ ] ConvertTo-Json 显式 Depth >= 6；单包与多包顶层 JSON 形状一致；records=[] 可正常解析。
- [ ] runtime_behavior=null / trust_level=null 可解析（Option<String>）；身份字段仍严格必填。
- [ ] 成功但空 stdout 是 APPX_DISCOVERY_PROTOCOL_INVALID；schema 不匹配是 APPX_DISCOVERY_PROTOCOL_UNSUPPORTED；解析失败 fail closed。
- [ ] Windows 集成测试对当前机器执行生产 discovery（tests/appx_discovery.rs）；TUI 启动无 APPX_DISCOVERY_INVALID 且 Entry 行显示选定入口。
- [ ] 已注册包只走原生应用激活（AO_NONE），绝不裸 EXE/调试上下文/提权；激活后核验目标 PackageFullName、AUMID、映像路径、创建时间与提升状态，不符或查询失败不返回成功回执。
- [ ] 激活 worker 有界协议（16 KiB 请求/64 KiB 回执）、提交前取消不激活、提交后按 outcome-unknown 报告；worker 超时/取消被有界回收，绝不终止 Desktop。
- [ ] TUI 与 `launch --json` 均显示产品类型、包名、版本、架构、发现来源与 `daemon_preparation`（skipped/stopped/not_needed），不输出安装路径或认证信息。
- [ ] 普通启动：不解析 Codex CLI、不执行任何 daemon 命令；非打包 override 注入环境；已注册应用原生激活并提交经校验的 Chromium 代理参数。
- [ ] `.env` 代理块默认关闭；仅 TUI `B`+`Y` 对显示的确切 Home 授权；未授权/作用域未确认/冲突（含 `export KEY=...` 写法）均如实报错，块外字节逐字保留；撤销先安全移除块、成功后才落盘关闭授权，revoke 失败时授权与绑定 Home 原样保留。
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

真实 Desktop 验收矩阵与结果记录在 [包身份验收表](PACKAGE_IDENTITY_ACCEPTANCE.md)。
标为 `NOT RUN` 的项目不能因编译、fixture 或 portable smoke 通过而勾选。旧的
`Invoke-CommandInDesktopPackage` 包上下文候选已退役，不得复测或恢复。

- [ ] A 开始菜单正常注册入口基线：真实 PID、PackageFullName、AUMID、TokenElevation、无身份弹窗。
- [ ] B `launch --activation-only`：与 A 同等身份观测；回执 `proxy_delivery = not_established`。
- [ ] C 普通原生激活 + Chromium 代理参数：身份核验通过、界面正常、参数已提交（不宣称网络已验证）。
- [ ] D `B` 授权后的首次启动：`.env` 受管块写入、Coverage 变为 current、无副作用本地任务可执行。
- [ ] E 端口切换 10808→7890：保存后 Coverage=stale，下一次启动同步 `.env` 后变 current。
- [ ] F 显式 daemon 修复（维护窗口）：未授权时 `D` 被 `BACKEND_PROXY_REQUIRED_FOR_REPAIR` 阻止；
      授权且 `.env` 可 prepare 时按"先 prepare 后 stop"顺序执行；故意制造 `.env` 冲突时
      daemon 必须未被 stop（已由集成测试锁定，实机复核一次）。
- [ ] G `B` 撤销事务（rc.6）：正常撤销后块删除、`manage=false`、`proxy_env_home=""`、
      Coverage 回到 `setup required`；手工在 Guard 块内加入额外键后再撤销，必须报
      `BACKEND_PROXY_REVOKE_FAILED` 且授权、绑定 Home 与 `.env` 完全不变（重试语义）。
- [ ] H `export` 冲突（rc.6）：`.env` 写入 `export HTTP_PROXY=...` 后启动/刷新，必须报告
      `BACKEND_PROXY_CONFIG_CONFLICT`（只含键名），文件不被修改、不追加 Guard 块。
- [ ] Guard 退出后 Desktop 仍运行；普通 Enter 不停止共享服务；包更新、带空格/中文路径的发行 EXE 完成 smoke。

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
