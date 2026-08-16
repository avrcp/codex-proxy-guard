# Codex Proxy Guard V2：Managed Subscription + JP/SG/US Node Benchmark 实施手册

> 目标仓库：`avrcp/codex-proxy-guard`
>
> 基线：当前 `main`
>
> 参考实现：`avrcp/NodeBrowser` 分支 `codex/tui-subscriptions`
>
> 产品目标：在保留现有 External Proxy 模式的前提下，增加 Managed Mode：直接加载机场订阅，只保留日本、新加坡、美国节点，通过受控 benchmark 选择最适合 ChatGPT Desktop / Codex 的节点，并启动由 Guard 自己拥有的 sing-box sidecar。

---

## 0. 最重要的产品约束

本项目不是通用代理客户端，也不是风控规避工具。

本阶段只解决：

```text
机场订阅
   ↓
只保留 JP / SG / US
   ↓
验证真实出口国
   ↓
测试网络稳定性
   ↓
JP 优先，其次 SG，再其次 US
   ↓
选中单一节点并锁定
   ↓
启动本地 sing-box mixed proxy
   ↓
向 ChatGPT Desktop / Codex 注入 HTTP_PROXY / HTTPS_PROXY
```

“JP 优先”的含义是保持日常网络环境一致、减少不必要的出口地区漂移；不得用于绕过服务资格、支付资格、地区限制或身份验证要求。

### 硬性地区策略

第一版固定：

```text
Allowed Regions = JP, SG, US
Preference      = JP > SG > US
```

不做 UI 配置，不允许其他地区参与 benchmark 或 selection。

Selection 必须是**字典序优先**，不是“全球 score 最大者优先”：

```text
如果存在 Healthy JP → 一定选 JP 中最高分
否则如果存在 Healthy SG → 选 SG 中最高分
否则如果存在 Healthy US → 选 US 中最高分
否则 → NoHealthyManagedNode
```

因此，即使：

```text
JP score = 82
SG score = 96
```

只要 JP 节点达到 Healthy 门槛，仍选择 JP。

---

# 1. 当前仓库边界必须先正式升级

当前 `AGENTS.md`、`README.md`、`docs/ARCHITECTURE.md` 和 `docs/SECURITY.md` 明确声明：

- 不管理代理软件；
- 不检测代理质量；
- 不访问 OpenAI 网络目标；
- 不读取 Credential Manager；
- 只接受已有 loopback HTTP/Mixed proxy。

因此第一步不是写 benchmark，而是升级架构契约。

不要在旧边界下偷偷增加网络探测。

## 新产品边界

Guard V2 支持两种模式：

```text
External Mode
  用户自己提供 127.0.0.1:<port>
  行为保持当前版本兼容

Managed Mode
  Guard 管理 subscription / node / benchmark / sing-box sidecar
  Desktop 仍然只获得 loopback HTTP proxy
```

Guard 仍然：

- 不修改 Windows 系统代理；
- 不使用 TUN/WFP/WinDivert；
- 不抓包；
- 不解密 TLS；
- 不读取 ChatGPT/Codex token、cookie、OAuth 或用户认证文件；
- 不调用 Codex app-server / private IPC；
- 不自动提交真实 Codex prompt 来测速；
- 不终止 ChatGPT Desktop；
- 不编辑 `~/.codex/config.toml`。

Managed Mode 新增且只新增：

- 订阅管理；
- 自己命名空间下的订阅 URL Secret；
- sing-box sidecar 生命周期；
- 受控 HTTPS / Geo benchmark；
- JP/SG/US selection。

---

# 2. 最终架构

新增一个独立 crate：

```text
crates/
├── proxy-guard-core/
├── proxy-guard-network/      NEW
├── proxy-guard-windows/
└── proxy-guard-app/
```

职责：

```text
proxy-guard-core
├── GuardConfig
├── Action / Effect / TaskResult
├── ManagedMode domain model
├── NodeId / SubscriptionId
├── CodexRegion
├── BenchmarkReport
├── NodeSelection
└── Redaction

proxy-guard-network
├── SubscriptionFetcher
├── SubscriptionParser
├── RegionHintClassifier
├── SubscriptionStore
├── NodeStore
├── SecretStore
├── sing-box discovery/config/runtime
├── ExitIdentityProbe
├── NodeBenchmarkService
├── BenchmarkStore
└── NodeSelector

proxy-guard-windows
├── APPX discovery
├── Desktop process detection
├── startup lock
├── environment injection
└── Desktop launch

proxy-guard-app
├── CLI
├── TUI
├── effect dispatch
└── External/Managed launch orchestration
```

不要把 NodeBrowser 整个 runtime crate 依赖进来。

第一版允许“按设计移植” NodeBrowser 已成熟的通用代码，避免 `codex-proxy-guard -> git dependency -> private NodeBrowser`。

---

# 3. 从 NodeBrowser 移植哪些代码

以 `NodeBrowser/codex/tui-subscriptions` 为参考。

优先移植并重新命名：

```text
crates/nodebrowser-core/src/model/node.rs
crates/nodebrowser-core/src/model/subscription.rs

crates/nodebrowser-runtime/src/subscription/fetcher.rs
crates/nodebrowser-runtime/src/subscription/parser.rs
crates/nodebrowser-runtime/src/subscription/service.rs

crates/nodebrowser-runtime/src/storage/node_store.rs
crates/nodebrowser-runtime/src/storage/subscription_store.rs

crates/nodebrowser-runtime/src/secret/subscription_secret.rs

crates/nodebrowser-runtime/src/network/config_builder.rs
crates/nodebrowser-runtime/src/network/runtime.rs

crates/nodebrowser-runtime/src/geo/resolver.rs
```

不要移植：

```text
BrowserProfile
CDP
Chrome runtime
Browser Identity
SessionController
Browser Recovery
PaymentPolicy
```

这些属于 NodeBrowser。

---

# 4. 核心 Domain Model

## 4.1 CodexRegion

在 `proxy-guard-core` 新增：

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CodexRegion {
    JP,
    SG,
    US,
}
```

提供固定 preference：

```rust
impl CodexRegion {
    pub const PREFERENCE: [Self; 3] = [Self::JP, Self::SG, Self::US];

    pub const fn priority(self) -> u8 {
        match self {
            Self::JP => 0,
            Self::SG => 1,
            Self::US => 2,
        }
    }
}
```

不增加 `Other`。

不在第一版做地区配置文件。

---

## 4.2 ManagedNode

建议保留 NodeBrowser `NodeSpec + SingBoxOutbound` 的安全边界：

```rust
pub struct ManagedNode {
    pub schema_version: u32,
    pub id: NodeId,
    pub subscription_id: SubscriptionId,
    pub name: String,
    pub region_hint: CodexRegion,
    pub outbound: SingBoxOutbound,
    pub remote_key: String,
    pub state: ManagedNodeState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

状态：

```rust
pub enum ManagedNodeState {
    Active,
    Stale,
}
```

要求：

- `SingBoxOutbound` 不允许用户 supplied `tag`；
- 拒绝 `direct/block/dns/selector/urltest` 等 lifecycle-owned outbound；
- `remote_key` 必须像 NodeBrowser 一样忽略 URL fragment/display name，使机场只改节点名称时 NodeId 不发生 churn；
- subscription 更新时已有节点保持 NodeId。

---

# 5. Subscription URL 安全边界

复用 NodeBrowser 的设计：

```text
Subscription metadata → 普通持久化
Raw URL / token       → Windows Credential Manager
```

Raw URL 不得：

- 写 TOML；
- 写 JSON；
- 写日志；
- 写 benchmark 文件；
- 出现在错误消息；
- 出现在 TUI；
- 出现在 `--json` 输出。

Credential namespace 固定，例如：

```text
CodexProxyGuard.Subscription
```

username/key：

```text
<SubscriptionId>
```

只允许按已知 SubscriptionId 读写自己的 secret。

禁止枚举或读取其他 Windows Credential Manager 条目。

---

# 6. Subscription Fetcher

直接参考 NodeBrowser：

```text
HTTPS only
connect timeout = 5 s
total timeout   = 15 s
redirect <= 3
response <= 5 MiB
system proxy disabled
HTTP credentials in URL rejected
```

这是下载机场配置，必须使用主机真实网络，不经过当前 Managed Node。

原因：如果旧节点已经坏掉，仍必须能够更新订阅。

第一版支持：

```text
VLESS
Trojan
Shadowsocks
SOCKS
```

未知协议：

```text
skip + count unsupported
```

不能因为一个未知协议导致整份订阅失败。

---

# 7. 只保留 JP / SG / US：RegionHintClassifier

Share link 本身通常没有可信 country 字段，所以 import 阶段只能进行**保守名称预筛**。

新增：

```rust
pub struct RegionHintClassifier;

impl RegionHintClassifier {
    pub fn classify(name: &str) -> Option<CodexRegion>;
}
```

## 7.1 支持 alias

JP：

```text
🇯🇵
JP
JPN
JAPAN
日本
东京 / 東京
TOKYO
大阪
OSAKA
```

SG：

```text
🇸🇬
SG
SGP
SINGAPORE
新加坡
狮城 / 獅城
```

US：

```text
🇺🇸
US
USA
UNITED STATES
美国 / 美國
LOS ANGELES
SAN JOSE
SEATTLE
NEW YORK
DALLAS
CHICAGO
```

短代码必须按 token/boundary 匹配，禁止裸 substring：

```text
"BUSINESS" 不能因为包含 US 被识别成 US
```

## 7.2 Import 规则

```text
parsed candidate
      ↓
RegionHintClassifier
      │
      ├─ JP → persist
      ├─ SG → persist
      ├─ US → persist
      └─ None → ignore
```

`SubscriptionSyncSummary` 增加：

```rust
pub ignored_region: usize,
```

例如：

```text
Fetched        85
Imported       27
Updated         3
Stale           1
Unsupported     5
IgnoredRegion  52
Failed          0
```

**不要保存 HK/TW/KR/DE/UK 等节点。**

---

# 8. Region Hint 不是最终事实

名字只能用于预筛。

真正进入候选池前必须通过真实出口 Geo 验证。

新增：

```rust
pub struct ExitObservation {
    pub ip: IpAddr,
    pub country: CodexRegion,
    pub observed_at: DateTime<Utc>,
}
```

可直接参考 NodeBrowser 的 `GeoResolver`：

```text
local managed mixed proxy
        ↓
HTTPS Geo endpoint
        ↓
actual exit IP + country
```

必须确保请求明确走本节点的 `127.0.0.1:<ephemeral>` proxy。

禁止 direct fallback。

验证：

```text
node.region_hint == observed.country
```

否则：

```text
Rejected::CountryMismatch
```

比如节点名称是：

```text
JP Tokyo Premium
```

实际出口：

```text
US
```

直接淘汰。

---

# 9. Managed sing-box Runtime

Guard V2 的关键是：Guard 不再依赖 v2rayN 提供端口。

Managed Mode 对一个候选 Node 启动一个临时 sing-box：

```text
sing-box
├── mixed inbound
│   127.0.0.1:<ephemeral>
└── one selected remote outbound
```

Config 必须：

```json
{
  "log": { "disabled": true },
  "inbounds": [
    {
      "type": "mixed",
      "tag": "guard-in",
      "listen": "127.0.0.1",
      "listen_port": 0,
      "set_system_proxy": false
    }
  ],
  "outbounds": [
    {
      "type": "...",
      "tag": "active-node"
    }
  ],
  "route": {
    "rules": [
      {
        "inbound": ["guard-in"],
        "action": "route",
        "outbound": "active-node"
      }
    ],
    "final": "active-node"
  }
}
```

实现时不要真的把 `listen_port: 0` 交给 sing-box；先由 Guard 进行 loopback ephemeral port reservation，释放后写入已知端口，再启动 sidecar。

每份配置启动前必须：

```text
sing-box check -c <config>
```

通过才允许：

```text
sing-box run -c <config>
```

Guard 必须持有 sidecar process handle。

`Drop` / shutdown / cancel 必须回收整个 sidecar process tree。

不得设置系统代理。

---

# 10. Benchmark 目标

Benchmark 不是测速软件。

目标是回答：

> “哪一个 JP/SG/US 节点最适合作为一个长时间 ChatGPT Desktop / Codex Session 的 HTTP proxy？”

因此第一版不测：

- ICMP ping；
- Speedtest 带宽；
- 下载大文件；
- 真正发送 Codex prompt；
- Token/账号接口；
- 私有 API；
- 自动登录；
- UDP/Voice。

第一版测：

```text
1. sing-box config validity
2. sidecar startup
3. actual exit country
4. HTTPS path reachability
5. repeated response-header latency
6. request success rate
7. p95
8. jitter
9. exit-IP stability
```

---

# 11. CodexPathProbe

新增：

```rust
pub trait CodexPathProbe {
    async fn probe(
        &self,
        proxy: LoopbackProxyEndpoint,
    ) -> Result<PathSample, NetworkError>;
}
```

建议目标：

```text
https://chatgpt.com/
```

使用一个轻量 HEAD 请求。

这里的 success 指：

```text
HTTP request 成功走完 proxy CONNECT/TLS 并收到 HTTP response headers
```

不要要求业务 HTTP status == 200。

即使服务器返回 3xx / 4xx，只要是正常 HTTP response，说明网络路径成立。

因为 benchmark 的目标是网络传输，不是绕过认证。

记录：

```rust
pub struct PathSample {
    pub header_latency: Duration,
    pub http_status: u16,
}
```

`http_status` 只用于诊断，不进入“账号可用”判断。

---

# 12. 两阶段 Benchmark

不要一开始对所有节点跑深度测试。

## Stage A：Quick Scan

对所有 JP/SG/US Active 节点执行：

```text
prepare sing-box config
      ↓
sing-box check
      ↓
launch sidecar
      ↓
wait local mixed ready
      ↓
Geo probe
      ↓
country match?
      ↓
1 × CodexPathProbe
      ↓
stop sidecar
```

Quick Scan 结果：

```rust
pub enum QuickVerdict {
    Candidate,
    ConfigRejected,
    SidecarFailed,
    CountryMismatch,
    NetworkFailed,
}
```

只把 `Candidate` 送入 Deep Scan。

## Stage B：Deep Scan

不要把全球所有候选一起按 score 取 Top-K。

按地区分别取 Quick Scan latency 最好的若干个，例如：

```text
JP Top 6
SG Top 3
US Top 3
```

如果某地区不足就全部进入。

每个 Deep candidate：

```text
launch one sidecar
      ↓
Geo observation #1
      ↓
5 × HEAD https://chatgpt.com/
间隔 300~500 ms
      ↓
Geo observation #2
      ↓
stop sidecar
```

这样一次完整 benchmark 通常只深测最多 12 个节点。

---

# 13. BenchmarkReport

```rust
pub struct BenchmarkReport {
    pub schema_version: u32,
    pub node_id: NodeId,
    pub node_fingerprint: String,

    pub expected_region: CodexRegion,
    pub verified_region: CodexRegion,

    pub first_exit_ip: IpAddr,
    pub second_exit_ip: IpAddr,
    pub exit_ip_stable: bool,

    pub attempts: u8,
    pub successes: u8,

    pub median_header_ms: u64,
    pub p95_header_ms: u64,
    pub jitter_ms: u64,

    pub score: u16,
    pub verdict: BenchmarkVerdict,
    pub measured_at: DateTime<Utc>,
}
```

`node_fingerprint` 必须基于实际 outbound canonical JSON/hash。

只要机场修改协议参数，就自动使旧 benchmark cache 失效。

---

# 14. Healthy Hard Gates

先 Gate，再 Score。

必须同时满足：

```text
sing-box check PASS
sidecar startup PASS
actual country == expected JP/SG/US
country observation #1 == observation #2
request success rate >= 80%
p95 response-header latency <= 5000 ms
sidecar 未中途退出
```

任何一项失败：

```rust
BenchmarkVerdict::Rejected(reason)
```

Rejected 节点不能参与 selection。

IP 改变但 country 不变：

```text
允许继续参与
但 score penalty
```

因为目标是保持国家一致，固定单一 IP 是加分项但不是绝对要求。

---

# 15. Score

只用于同一地区 Healthy 节点之间排序。

第一版固定：

```text
Reliability / success rate   45 points
Median latency               25 points
P95 latency                  20 points
Jitter                       10 points
```

如果出口 IP 在两次观测间改变：

```text
score -= 8
```

最后 clamp：

```text
0..100
```

不要把国家 preference 混入 score。

国家 preference 在 `NodeSelector` 外层处理。

---

# 16. NodeSelector：JP > SG > US

实现必须非常简单：

```rust
pub fn select_best(reports: &[BenchmarkReport]) -> Option<NodeSelection> {
    for region in CodexRegion::PREFERENCE {
        if let Some(report) = reports
            .iter()
            .filter(|r| r.verdict.is_healthy())
            .filter(|r| r.verified_region == region)
            .max_by_key(|r| r.score)
        {
            return Some(NodeSelection::from(report));
        }
    }
    None
}
```

不要：

```text
JP score × 1.1
SG score × 1.0
US score × 0.9
```

这种加权仍可能被极端分数反超，不符合“JP 硬优先”。

---

# 17. Benchmark Cache

路径建议：

```text
%APPDATA%/codex-proxy-guard/
├── config.toml
├── managed/
│   ├── subscriptions/
│   ├── nodes/
│   └── benchmarks/
└── runtime/
```

Benchmark cache：

```text
managed/benchmarks/<node-id>.json
```

TTL 第一版：

```text
6 hours
```

Cache 可用条件：

```text
now - measured_at <= TTL
AND node_fingerprint == current fingerprint
AND node.state == Active
```

启动时逻辑：

```text
有 fresh Healthy cache?
      │
      ├─ YES
      │    ↓
      │ select JP > SG > US
      │    ↓
      │ 对 winner 做 1 次 Quick Recheck
      │    ↓
      │ PASS → launch
      │ FAIL → invalidate winner → benchmark
      │
      └─ NO
           ↓
        benchmark
```

不要每次启动全量 benchmark。

---

# 18. GuardConfig v3

当前 schema 是 version 2。

V2 Managed Mode 建议升：

```toml
version = 3

[proxy]
mode = "external"
scheme = "http"
host = "127.0.0.1"
port = 10808
no_proxy = ["localhost", "127.0.0.1", "::1"]

[managed]
enabled = false
subscription_id = ""
benchmark_cache_hours = 6

[codex]
executable_override = ""
refuse_if_running = true

[tui]
alternate_screen = "auto"
```

`proxy.mode`：

```rust
pub enum ProxyMode {
    External,
    Managed,
}
```

External：

- 保留当前行为；
- `host` 必须 loopback；
- `scheme=http`；
- 不启动 sing-box。

Managed：

- 忽略用户手写 proxy.port；
- 自动启动 ephemeral local mixed proxy；
- `managed.subscription_id` 必须存在；
- 如果没有 Healthy node，不启动 Desktop。

仍然按照仓库现有策略：

```text
旧 schema 不静默迁移
```

升级时通过：

```text
codex-proxy-guard init-config --force
```

或实现明确的 `migrate-config-v2` 子命令；不要静默 rewrite。

---

# 19. Managed Launch Pipeline

External Mode 保持：

```text
discover Desktop
  ↓
startup lock
  ↓
launch with configured loopback proxy
```

Managed Mode：

```text
validate config
      ↓
load selected subscription
      ↓
sync?（仅用户请求或过期策略，不要每次强制）
      ↓
load benchmark cache
      ↓
select fresh Healthy node (JP > SG > US)
      │
      ├─ none → benchmark
      │
      └─ found → quick recheck winner
      ↓
start persistent managed sing-box for winner
      ↓
obtain http://127.0.0.1:<ephemeral>
      ↓
discover Desktop
      ↓
startup lock
      ↓
inject HTTP_PROXY/HTTPS_PROXY
      ↓
launch Desktop
      ↓
Guard keeps sing-box handle
```

如果 Desktop launch 失败：

```text
stop sing-box
return error
```

不能遗留 sidecar。

---

# 20. Managed Runtime Lifecycle

现有产品原则“Guard 不终止 Desktop”继续保持。

Managed Mode 下：

```text
Guard owns sing-box
Guard does NOT own Desktop lifetime
```

如果用户按 Q：

```text
if managed sidecar active:
    prompt confirmation
    stop sing-box
    Guard exit
    Desktop remains open
```

明确提示：

```text
The managed proxy will stop. ChatGPT Desktop remains open.
```

不要强杀 Desktop。

如果 sing-box 意外退出：

```text
ManagedProxyLost
```

TUI 高亮错误。

第一版不要自动切节点。

也不要在同一个 Desktop Session 中热切：

```text
JP-A → JP-B
```

即使 B benchmark 更高，也要等下一次 Desktop Session。

---

# 21. Action / Effect / TaskResult 扩展

必须继续保持当前：

```text
Action
→ candidate reduce
→ authorize
→ commit
→ dispatch
→ TaskResult
```

不要绕开 reducer 直接在 TUI 里做网络操作。

新增 Intent：

```rust
UserIntent::SyncSubscription
UserIntent::BenchmarkNodes
UserIntent::LaunchManaged
UserIntent::CancelBenchmark
UserIntent::SelectSubscription
```

新增 Effect：

```rust
AppEffect::SyncSubscription(SubscriptionId)
AppEffect::BenchmarkNodes(SubscriptionId)
AppEffect::LaunchManaged(NodeId)
AppEffect::StopManagedProxy
```

新增 TaskResult：

```rust
SubscriptionSynced(Result<SubscriptionSyncSummary, String>)
BenchmarkCompleted(Result<BenchmarkRunSummary, String>)
ManagedLaunchCompleted(Result<ManagedLaunchReceipt, String>)
ManagedProxyStopped(Result<(), String>)
```

Capabilities 增加：

```rust
pub struct Capabilities {
    pub launch_process: bool,
    pub save_config: bool,
    pub manage_subscription: bool,
    pub benchmark_network: bool,
    pub manage_sidecar: bool,
    pub quit: bool,
}
```

任意时刻仍然只允许一个 foreground operation。

---

# 22. CLI 设计

订阅 URL 很长，不建议在 Guard TUI 里做复杂编辑器。

第一次配置用 CLI：

```powershell
codex-proxy-guard subscription add `
  --name "My Airport" `
  --url "https://..."
```

然后：

```powershell
codex-proxy-guard subscription list
codex-proxy-guard subscription sync "My Airport"
codex-proxy-guard subscription delete "My Airport" --yes
```

查看导入后的三地区节点：

```powershell
codex-proxy-guard node-list
codex-proxy-guard node-list --region JP
codex-proxy-guard node-list --region SG
codex-proxy-guard node-list --region US
```

Benchmark：

```powershell
codex-proxy-guard benchmark
codex-proxy-guard benchmark --json
```

推荐节点：

```powershell
codex-proxy-guard best-node
```

输出：

```text
JP Tokyo Premium
Region: JP
Score: 93
Success: 100%
Median: 84 ms
P95 (5 samples): 121 ms
Exit: stable
```

JSON 不得包含 outbound credential、UUID、password、subscription URL。

---

# 23. TUI 设计

Guard 的 TUI 继续保持单屏，不做大型代理客户端。

建议：

```text
┌──────────────────────────────────────────────────────────────┐
│ Codex Proxy Guard                           MANAGED ● READY  │
├──────────────────────────────┬───────────────────────────────┤
│ DESKTOP                      │ SELECTED NODE                 │
│                              │                               │
│ ChatGPT Desktop    FOUND     │ JP Tokyo 01                   │
│ Process            STOPPED   │ Region       JP VERIFIED      │
│                              │ Score        93               │
│ Proxy Mode         MANAGED   │ Success      100%             │
│ Subscription       Airport   │ Median       84ms             │
│                              │ P95          121ms            │
├──────────────────────────────┴───────────────────────────────┤
│ REGIONS                                                      │
│ JP   8 active   6 healthy                                    │
│ SG   5 active   4 healthy                                    │
│ US   7 active   5 healthy                                    │
├──────────────────────────────────────────────────────────────┤
│ [Enter] Launch  [B] Benchmark  [S] Sync  [R] Refresh  [?]   │
└──────────────────────────────────────────────────────────────┘
```

TUI 不输入 subscription URL。

按 `S` 只同步已经保存的 subscription。

按 `B` 跑 benchmark。

按 `Enter`：

```text
fresh winner → launch
cache stale  → 提示先 benchmark，或自动执行 benchmark effect 后继续
```

---

# 24. Benchmark 并发策略

第一版优先正确性。

Quick Scan 可以最多并发：

```text
3 nodes
```

每个节点必须：

- 独立 ephemeral mixed port；
- 独立 config dir；
- 独立 sidecar handle；
- 独立 cancellation child token。

Deep Scan 第一版建议串行，避免同时制造大量目标请求。

不能对 `chatgpt.com` 做几十路高并发探测。

每次完整 benchmark 应保持低频、低请求量。

---

# 25. Cancellation

现有 Guard 已有 `CancellationToken`，必须继续使用。

Benchmark：

```text
每个 node step 前检查 cancellation
每个网络 request 有 timeout
每个 sing-box check 有 timeout
每个 child process 可回收
```

用户按 Cancel / Q：

```text
cancel benchmark
↓
stop all benchmark sidecars
↓
return TaskResult
```

不能遗留 sing-box.exe。

---

# 26. 日志与脱敏

允许：

```text
Benchmark JP Tokyo 01: 93
Geo country: JP
HTTPS probe: 84 ms
```

禁止：

```text
vless://UUID...
subscription token
password
Reality public share link
完整 outbound JSON
```

节点 display name 可以显示。

NodeId 可以显示短 ID。

所有外部错误进入 TUI/CLI 前走现有 redaction boundary。

---

# 27. PR 切分

不要一个 PR 一次性重写全部。

## PR-01 — Architecture Boundary V3

目标：只改变正式产品契约和 domain，不启动 sing-box。

修改：

```text
AGENTS.md
README.md
docs/ARCHITECTURE.md
docs/SECURITY.md
GuardConfig version 3
ProxyMode
CodexRegion
Managed config models
Action/Effect/Capabilities skeleton
```

验收：当前 External Mode 行为测试完全通过。

---

## PR-02 — Subscription + Region Filter

移植：

```text
SubscriptionSource
SubscriptionFetcher
SubscriptionParser
SubscriptionStore
NodeStore
SecretStore
SubscriptionService
```

新增：

```text
RegionHintClassifier
ignored_region
JP/SG/US only persistence
```

验收：订阅里即使有 80 个全球节点，最终 Store 中只能出现 JP/SG/US。

---

## PR-03 — Managed sing-box Runtime

实现：

```text
SingBoxLocator
LoopbackPortReservation
SingBoxConfigBuilder
sing-box check
sidecar spawn
mixed readiness
bounded shutdown
```

验收：给一个 Node 能得到：

```text
http://127.0.0.1:<ephemeral>
```

且退出后无 orphan process。

---

## PR-04 — Geo Verify + Benchmark + Selector

实现：

```text
ExitIdentityProbe
CodexPathProbe
Quick Scan
Deep Scan
BenchmarkReport
BenchmarkStore
NodeSelector
JP > SG > US
```

验收：国家不匹配永远不可能被选中。

---

## PR-05 — Managed Launch Pipeline

把 winner sidecar 与 Desktop launch 串起来。

实现：

```text
managed launch effect
sidecar active state
managed launch receipt
launch rollback
shutdown sidecar
```

验收：Desktop 的注入 proxy 必须是 Guard 新启动的 loopback endpoint。

---

## PR-06 — TUI / CLI / Hardening

实现：

```text
subscription CLI
node-list CLI
benchmark CLI
best-node CLI
TUI managed status
benchmark progress
cancel
portable build
release docs
```

---

# 28. 单元测试要求

## Region classifier

```text
JP / JPN / 日本 / 東京 / Tokyo → JP
SG / Singapore / 新加坡       → SG
US / USA / 美国 / Seattle     → US
BUSINESS                       → None
Germany                        → None
HK                             → None
```

## Subscription

```text
URL never serialized
URL never logged
HTTPS required
Base64 subscription
VLESS Reality parse
Trojan parse
SS parse
SOCKS parse
remote key ignores fragment rename
unsupported skipped
non JP/SG/US ignored
stale preserved
sync failure keeps last-known-good
```

## sing-box

```text
config check before run
mixed loopback only
set_system_proxy=false
outbound tag owned by builder
invalid config never starts run
startup early exit fails
shutdown reaps process
cancellation reaps process
```

## Geo

```text
JP hint + JP actual → pass
JP hint + US actual → reject
SG hint + SG actual → pass
US hint + US actual → pass
actual KR → reject
```

## Benchmark

```text
failed probe lowers reliability
p95 calculated correctly
jitter deterministic
IP change applies penalty
country change rejects
sidecar exit rejects
cache fingerprint mismatch invalidates
cache TTL invalidates
```

## Selector

必须特别测试：

```text
JP 70 vs SG 99 → JP
JP none, SG 80, US 99 → SG
JP rejected, SG 80 → SG
JP rejected, SG rejected, US 75 → US
all rejected → None
```

---

# 29. 集成测试

使用 fake sing-box fixture，不依赖真实机场。

至少覆盖：

```text
subscription → region filter → NodeStore
Node → generated config → fake sing-box check/run
benchmark fake probe → cache
selector → managed endpoint
managed endpoint → launch config environment
```

不要让 CI 真实访问机场、chatgpt.com 或 Geo provider。

所有外部网络必须通过 trait + fake implementation 替换。

---

# 30. 手工实机验收

完成代码后，在 Windows 真实环境执行。

## Test A：真实订阅

```powershell
codex-proxy-guard subscription add --name "Airport" --url "<real URL>"
codex-proxy-guard subscription sync "Airport"
```

确认：

```text
Store 里只有 JP / SG / US
没有 HK / TW / KR / DE 等
```

## Test B：真实 benchmark

```powershell
codex-proxy-guard benchmark
```

确认：

```text
每个候选都有 verified region
country mismatch 被 Reject
无遗留 sing-box.exe
```

## Test C：JP 优先

准备：

```text
JP Healthy 80
SG Healthy 95
US Healthy 97
```

结果必须：

```text
JP selected
```

## Test D：JP 全失败

结果：

```text
SG selected
```

## Test E：JP/SG 全失败

结果：

```text
US selected
```

## Test F：全部失败

结果：

```text
Desktop 不启动
明确提示 NoHealthyManagedNode
```

## Test G：缓存

第一次 full benchmark 后立即重启 Guard：

```text
不应再次 full scan
只 quick recheck winner
```

## Test H：Node update

机场修改 winner outbound 后 sync：

```text
old benchmark fingerprint invalid
下次必须重新 benchmark
```

## Test I：Managed sidecar

启动 Desktop 后检查：

```text
Desktop env proxy == 127.0.0.1:<managed ephemeral>
```

手动结束 sing-box：

```text
TUI 显示 ManagedProxyLost
不得静默热切其他节点
```

---

# 31. 质量门禁

每个 PR 完成前：

```powershell
codegraph status .

cargo fmt --all -- --check

cargo clippy `
  --workspace `
  --all-targets `
  --locked `
  -- `
  -D warnings

cargo test `
  --workspace `
  --all-targets `
  --locked

cargo audit

.\scripts\build-portable.cmd
```

最终检查：

```text
portable EXE
SHA-256
build-info.json
```

全部保持现有 release 规范。

---

# 32. Codex 执行约束

开始实施前必须完整阅读：

```text
AGENTS.md
README.md
docs/ARCHITECTURE.md
docs/SECURITY.md
crates/proxy-guard-core/src/action.rs
crates/proxy-guard-core/src/reducer.rs
crates/proxy-guard-core/src/config.rs
crates/proxy-guard-app/src/dispatcher.rs
crates/proxy-guard-windows/src/*
```

同时参考 NodeBrowser：

```text
core/model/node.rs
core/model/subscription.rs
runtime/subscription/*
runtime/storage/node_store.rs
runtime/storage/subscription_store.rs
runtime/secret/subscription_secret.rs
runtime/network/config_builder.rs
runtime/network/runtime.rs
runtime/geo/resolver.rs
```

执行原则：

1. 不绕开现有 reducer / capability / effect 边界。
2. 不删除 External Mode。
3. 不把 Guard 改成系统代理/VPN/TUN。
4. 不访问 Codex 私有 API。
5. 不读取账号 Token/Cookie。
6. 不使用真实 prompt 做 benchmark。
7. 不高频轰炸 OpenAI endpoint。
8. 不在 active Desktop Session 内自动热切节点。
9. 不允许 JP/SG/US 以外节点进入 Managed NodeStore。
10. 不允许 country mismatch 节点成为 winner。
11. 不把 subscription secret 写入普通文件或日志。
12. 不允许 orphan sing-box process。
13. 不强杀 Desktop。
14. 不在一个 PR 同时改完全部 V2。
15. 每个 PR 更新文档和测试。

---

# 33. Definition of Done

只有以下全部完成，V2 才算完成：

```text
[ ] External Mode 无回归
[ ] Managed Mode 可加载 subscription
[ ] Subscription URL 安全保存
[ ] 只导入 JP / SG / US
[ ] VLESS Reality 可工作
[ ] Trojan / SS / SOCKS 可工作
[ ] 真实出口国验证
[ ] Country mismatch 永久淘汰
[ ] Quick Scan 工作
[ ] Deep Scan 工作
[ ] Benchmark cache 工作
[ ] cache 与 node fingerprint 绑定
[ ] JP Healthy 时始终选 JP
[ ] JP 无 Healthy 时 fallback SG
[ ] SG 无 Healthy 时 fallback US
[ ] 全失败时拒绝启动 Desktop
[ ] winner 启动独立 sing-box sidecar
[ ] Desktop 只收到 loopback proxy endpoint
[ ] Guard 保持 sidecar ownership
[ ] sidecar 意外退出可见
[ ] active Session 不自动热切 node
[ ] Guard quit 回收 sidecar
[ ] Guard 不终止 Desktop
[ ] CLI 可以 sync / benchmark / best-node
[ ] TUI 可显示 region / score / winner
[ ] 所有错误经过 redaction
[ ] fmt PASS
[ ] clippy PASS
[ ] tests PASS
[ ] cargo audit PASS
[ ] portable build PASS
```

---

# 34. 最终产品行为

日常用户体验应收敛到：

```text
启动 codex-proxy-guard.exe
        ↓
读取 Managed Mode
        ↓
读取 fresh benchmark cache
        ↓
JP 有 Healthy?
        │
        ├─ YES → JP best
        │
        └─ NO
             ↓
          SG 有 Healthy?
             │
             ├─ YES → SG best
             │
             └─ NO → US best
        ↓
Quick Recheck winner
        ↓
启动 persistent sing-box
        ↓
127.0.0.1:<ephemeral>
        ↓
Launch ChatGPT Desktop / Codex
```

正常情况下用户看到：

```text
Managed Mode
Subscription  Airport
Selected      JP Tokyo 01
Region        JP VERIFIED
Score         93
Proxy         127.0.0.1:41823
Desktop       READY

[Enter] Launch
```

这就是 V2 的最终目标：

> 一个只关心 JP / SG / US、JP 硬优先、以真实出口国和重复 HTTPS 稳定性为依据选节点、并为 ChatGPT Desktop / Codex 提供自管理 loopback proxy 的轻量 Guard。
