# Architecture

## 单一职责

Codex Proxy Guard 仅为新启动的 ChatGPT Desktop（Chat、Work 和 Codex）进程树注入本机
loopback HTTP/Mixed 代理环境。它不接管 Desktop 生命周期，也不强制隧道流量。

V2 提供两种模式：

- **External Mode**（默认）：用户提供 `127.0.0.1:<port>`，行为与 V1 兼容。
- **Managed Mode**：Guard 加载机场订阅，只保留 JP/SG/US 节点，通过受控 benchmark
  选出 JP 硬优先的节点，并启动 Guard 自有的 sing-box sidecar。Desktop 仍然只收到
  `HTTP_PROXY`/`HTTPS_PROXY` 指向 `127.0.0.1:<ephemeral>`。

```text
External:
  校验配置 → 发现 Desktop → 启动锁 → 确认未运行 → 注入代理 → spawn → LaunchReceipt

Managed:
  校验配置 → 读订阅 → 读 benchmark cache
       → 选 fresh Healthy (JP > SG > US)
       → 启动 sing-box → ready → Geo + ChatGPT HEAD 复验
       → 保留同一个已验证 sidecar → 127.0.0.1:<ephemeral>
       → 发现 Desktop → 启动锁 → 注入代理 → spawn → Guard 持有 sidecar handle
```

## Crate 边界

- `proxy-guard-core`：配置、领域状态、Action/Effect/TaskResult、reducer、脱敏，以及
  Managed Mode 领域模型（`CodexRegion`、`NodeId`/`SubscriptionId`、`ManagedNode`、
  `BenchmarkReport`、`NodeSelection`）。无终端/网络/进程/Windows 依赖。
- `proxy-guard-network`：订阅 fetch/parse/region-filter/storage、Credential-Manager
  secret 边界、sing-box 发现/配置/运行、出口 Geo 验证、两阶段 benchmark、cache 与
  JP/SG/US selection。
- `proxy-guard-windows`：APPX 发现、Desktop 根进程检测、启动锁、环境注入与进程启动。
- `codex-proxy-guard`：CLI、单屏 TUI、effect dispatch、External/Managed 启动编排，以及
  按需启动的 Local Web Manager 适配器。它拥有两个平面：运行时平面
  （TUI → EffectDispatcher → verified sidecar → Desktop）与管理平面
  （浏览器 → loopback Axum manager → 共享 network services）。

所有会改变外部状态的操作都遵循：

```text
Action → candidate reduce → authorize → commit → dispatch → TaskResult
```

任意时刻只允许一个前台操作。Benchmark 可被 CancelBenchmark effect 取消。

## Local Web Manager（双平面）

Web Manager 是 `codex-proxy-guard` 的第三个 presentation adapter，与 CLI、TUI 并列，
不是常驻 daemon，也不属于 `proxy-guard-network`。

```text
RUNTIME PLANE                 MANAGEMENT PLANE
    TUI                           Default Browser
     │                                 │
     ▼                         127.0.0.1:<ephemeral>
EffectDispatcher                       ▼
     │                             Axum Local Manager
     ▼                                 │
verified sidecar                 ManagedOperations
     │                          (subscription/node/benchmark)
     ▼
Desktop
```

- `managed_services.rs` 提供 CLI/TUI/Web 共用的 service factory（`subscription_service`、
  `benchmark_service`、`node_store`、`load_managed_view`）；Web 路由不自行构造 service。
- Dispatcher 持有唯一的 `ManagerHandle`。`M` 打开时先满足互斥门（无 foreground、
  Desktop 未运行、无 managed sidecar），再绑定 `127.0.0.1:0`、生成 256-bit 会话 token、
  用默认浏览器打开 `/#token=<secret>`；`O` 重开标签页，`M` 关闭。
- `shutdown()` 顺序：cancel benchmark → 停止 Manager server → 停止 managed sidecar。
- Manager 只做低频 HTTP JSON + 轮询（无 WebSocket/SSE）；benchmark 与 sync 共用
  `Semaphore(1)`，忙时返回 `409 OPERATION_BUSY` 而不排队。
- Web 激活订阅时：克隆 config → `proxy.mode = managed` → 写入 `managed.subscription_id`
  → validate → save → 发送 `ManagerConfigUpdated`，reducer 立即替换 `state.config`；
  Manager 关闭后再触发 `RefreshLocalState`，避免使用旧的内存配置。
- 手选节点（`healthy_selection_for` 通过后才允许）只存于本会话；启动解析
  `manual → auto (JP > SG > US)`，最终都必须经过 `start_verified_sidecar` 复验。

## Managed Mode 数据流

```text
订阅 URL（仅 Credential Manager）
   ↓ HttpsSubscriptionFetcher（主机真实网络，HTTPS-only，5 MiB 上限）
SubscriptionParser（VLESS/Trojan/SS/SOCKS）
   ↓ RegionHintClassifier（名称预筛 JP/SG/US）
SubscriptionService（事务 reconcile，remote_key 去重，stale 标记）
   ↓ NodeStore（ManagedNode，SingBoxOutbound 无 tag）
NodeBenchmarkService
   ↓ Quick Scan（并发 ≤3：check → launch → ready → Geo → 1×HEAD）
   ↓ Deep Scan（串行：Geo#1 → 5×HEAD → Geo#2；JP Top6/SG Top3/US Top3）
   ↓ BenchmarkReport（硬门禁 + score，fingerprint 绑定）
BenchmarkStore（TTL + fingerprint + expected region 失效；损坏缓存按 miss 重算）
   ↓ NodeSelector（JP > SG > US，字典序）
   ↓ 同一个 sidecar 做启动前 Geo/HEAD 复验 → Desktop 环境注入
```

## 配置与 TUI

配置文件只接受当前 schema（v3）；旧版本和未知字段会被拒绝，不会迁移或静默忽略。无子命令
启动时，若配置无效，应用进入配置修复界面而不是退出：按 `C` 打开编辑器，`Ctrl-U` 清空，
`Tab`/方向键切换 host 与 port，`Enter` 保存，`Esc` 放弃。

Managed Mode 下 TUI 保持单屏，显示订阅名、各区域 active/healthy 计数与选中节点。按 `S`
同步订阅，`B` 跑 benchmark，`Enter` 启动。TUI 不输入订阅 URL；订阅通过 CLI
`subscription add` 或浏览器管理界面（`M`）配置。TUI 只负责"看状态 + Launch + Manage"；
订阅/节点/benchmark/下一启动选节点的低频表单化操作交给浏览器。

## 启动互斥

从进程快照到 spawn 期间，Guard 持有当前用户临时目录内的排他文件锁。Windows 使用
`share_mode(0)`，防止两个 Guard 同时通过“未运行”检查。spawn 成功后仅记录短暂时间戳以
覆盖新进程尚未出现在快照中的竞态；Guard 退出绝不终止 Desktop。

## APPX 发现与环境

优先使用 `codex.executable_override`，其次使用当前运行期缓存，最后通过受限且有超时的
PowerShell 查询当前 ChatGPT Desktop 与 ChatGPT Classic 的已知 APPX 包。发现阶段只从同一
产品内选择最高版本；选择阶段固定优先当前 ChatGPT Desktop，Classic 仅作后备。

新进程树继承现有环境，并仅覆盖 `HTTP_PROXY`/`HTTPS_PROXY`（两套大小写）与
`NO_PROXY`/`no_proxy`，同时移除 `ALL_PROXY`/`all_proxy`。External Mode 的端点是配置值，
Managed Mode 的端点是 Guard 刚启动的 sidecar loopback 端点。

## Managed sidecar 生命周期

Guard 拥有 sing-box 进程树（Windows 上通过 Job Object 保证回收），但不拥有 Desktop。
按 `Q` 退出时显式等待 sidecar 回收，Desktop 保持打开。sidecar 意外退出时 TUI 显示
`ManagedProxyLost`，不自动热切节点；同一 Desktop Session 内不切换节点。
