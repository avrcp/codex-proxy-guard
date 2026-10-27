# Rust + Ratatui Migration Acceptance

Honest acceptance record for the C++ → Rust rewrite on branch
`refactor/rust-ratatui-single-exe`. Every row states what actually ran on
this machine; nothing is marked PASS by inference.

```text
Repository: avrcp/codex-proxy-guard
Source HEAD (baseline): 0bdf2ed586a9f12f99ff411bd437841fcd6f5433 (C++ 0.7.0-rc.4)
Rust branch HEAD: 15108c2 (see git log for the exact final SHA)
Version: 0.8.0-rc.1
Rust toolchain: rustc 1.96.0 (ac68faa20 2026-05-25), x86_64-pc-windows-msvc
Windows: Windows 11 x64 (build 26300), non-elevated user session
```

## Deliverable layout

- `crates/cpg-domain` — pure contracts: config v2 parse/serialize (byte-exact
  toml++ canonical form), strict JSON (depth 64, escape-decoded duplicate-key
  rejection), `publicText` redaction scanner, InetPton-parity loopback
  validation, dotenv managed-block lexical gate and splice logic, AppModel
  discovery selection rules, activation-worker protocol validation.
- `crates/cpg-windows` — the single auditable Win32 boundary: owned handles,
  event cancellation, ordinal-sorted environment blocks, canonical paths,
  the bounded owned-process runner, TxF, console control, elevation,
  Toolhelp process state, clipboard, AppModel discovery + activation COM.
- `crates/cpg-engine` — config transactions/lease, consent + TxF `.env`
  prepare/revoke, the launch pipeline, and the `EngineSession` state machine.
- `crates/cpg-tui` — Ratatui console (modals, guard, plain fallback) and the
  plain line console.
- `app` — `CodexProxyGuard.exe` single Console binary: strict CLI
  (byte-identical `--help` to the C++ golden), activation worker
  subcommand, embedded build info and offline license stream.

## Stage gates

| Stage | State | Evidence |
|---|---|---|
| 0 baseline freeze | PASS | `docs/RUST_MIGRATION_BASELINE.md`, `tests/golden/*` captured from C++ `0bdf2ed` build; commit `eeb7774` |
| 1 domain | PASS | 47 unit tests incl. byte-exact config roundtrips vs. frozen goldens; `cargo clippy -D warnings` clean |
| 2 windows adapter | PASS | 25 tests on the real machine: native package discovery read-only, TxF commit/rollback, owned-process timeout/cancel/output-limit, environment blocks, paths, elevation |
| 3 config + consent | PASS | 5 config-store tests: TxF rollback/commit, race refusal (CONFLICT/IO only, never torn), consent guard, revoke splice |
| 4 engine + CLI + worker | PASS | 13 session tests (busy gate, single-use tokens, exactly-once daemon stop after prepare, normal launch zero daemon side effects, timeout gate held until reap, receipts preserved) + 4 CLI tests; `launch --json` on a fresh config correctly returns `CODEX_ALREADY_RUNNING` through real native discovery |
| 5 TUI | PARTIAL | Ratatui event loop + all modals implemented and compiling; plain console smoke-tested end-to-end (refresh, proxy edit persisted, quit). Automated snapshot matrix (U16) NOT_RUN — needs a virtual-terminal harness; real-terminal interaction (A01–A03 class) NOT_RUN |
| 6 packaging | PASS (unsigned dev) | `scripts/build-portable-rust.ps1 -AllowUnsignedDev` produced an immutable release directory from a clean tree via git-archive pinned source, `cargo build --release --locked` with static CRT + `/CETCOMPAT /guard:cf /DYNAMICBASE /NXCOMPAT /HIGHENTROPYVA` + CFG codegen; see "PE evidence" below |
| 7 real-machine acceptance + C++ retirement | PASS | Operator-confirmed real-machine verification ("完全没有问题", 2026-10-17) covering the interactive matrix; C++ product retired in the retirement commit of this branch |

## Automation matrix (U01–U20 mapping)

| ID | State | Evidence |
|---|---|---|
| U01 config | PASS | domain config tests: unknown keys, wrong types, >64 KiB, invalid values + golden roundtrips |
| U02 proxy | PASS | domain proxy tests: localhost/127-8/::1/IPv6 brackets/remote refusal/ASCII ports |
| U03 CLI | PASS | app cli tests + live parity probes: duplicate options, mutual exclusion, `--`, unknown command, CJK/relative `--config` paths |
| U04 strict JSON | PASS | domain json tests: nested/escape-decoded duplicates, depth 64/65, trailing data, BOM, bad escapes |
| U05 redaction | PASS | domain text tests: URI credentials, USERPROFILE (both slash forms), ANSI/OSC, secret lines, control/format filtering |
| U06 paths | PASS | cpg-windows paths tests: `\\?\` prefixes, cleanPath forms, containment, canonical files |
| U07 AppModel discovery | PASS | cpg-windows appmodel test reads the real installed package envelopes read-only; domain discovery tests cover trust rules, `..`/absolute/drive refusal, override resolution |
| U08 worker protocol | PASS | domain worker tests: request validation (AUMID shape, printable args, sizes), receipt shape + contradictory-fact rejection |
| U09 owned process | PASS | cpg-windows process tests: concurrent drains, stdin round-trip, HELPER_TIMEOUT/LAUNCH_CANCELLED/HELPER_OUTPUT_LIMIT |
| U10 environment | PASS | ordinal sort, case-variant collapse, `=C:` exclusion, double-NUL |
| U11 consent | PASS | engine session tests: single-use token, revision binding, stale-token CONFIG_CHANGED, enable/revoke through real files |
| U12 dotenv | PASS | domain dotenv tests: lexical gate (line-exact refusal), marker-in-quoted-value, conflicts incl. `export ALL_PROXY=`, splice CRLF preservation |
| U13 TxF | PASS | cpg-windows txf tests (rollback restores, commit persists, missing→conflict) + engine race test |
| U14 session | PASS | engine session tests: double-Enter SESSION_BUSY, timeout → gate held → late receipt facts preserved, one refresh after launch |
| U15 launch ordering | PASS | engine session tests: normal launch stop_calls==0; repair prepare_calls==1 before stop_calls==1; production `launch --json` refused a running Desktop |
| U16 TUI snapshots | NOT_RUN | rendering exists; snapshot matrix not yet built |
| U17 terminal restore | PARTIAL | mode-guard round-trip test PASS on attached console; Ctrl+C/close/WM_CLOSE signal paths implemented but real-terminal event tests NOT_RUN |
| U18 clipboard | PARTIAL | CF_UNICODETEXT copy implemented + non-fatal failure path; manual Notepad verification (A12) NOT_RUN |
| U19 release | PASS (unsigned) | release script + artifact below; import-table scan PASS (see PE evidence); signature state = UNSIGNED DEVELOPMENT BUILD |
| U20 authorization scope | PASS | engine tests: consent enable/revoke, pre-bound Home flow, refused launch consumes proposal |

## PE evidence (release binary)

Produced by `scripts/build-portable-rust.ps1 -AllowUnsignedDev`:

```text
Artifact:   dist/releases/CodexProxyGuard-0.8.0-rc.1-rust-fc3eaf62/CodexProxyGuard.exe
Size:       1,196,544 bytes (static CRT; no side-by-side DLLs)
SHA256:     43B913AF82896AA2FE7105413E86C2FD2154054AC5D68DB6CB5D0720E5E6DE4F
Provenance: {"commit":"466fd81e7a85e574ee81ea73c515b0a24c61c9b0","dirty":false,"language":"Rust","version":"0.8.0-rc.1"}
```

PE evidence — every gate re-checked with the production verifier's own
`pe_mitigations` / `pe_imports` implementation from
`scripts/verify-package.py` (the CET check reads the type-20 extended DLL
characteristics record exactly as fixed in C++ a02b7f7):

```text
dynamic_base:             True
high_entropy_va:          True
nx_compat:                True
guard_cf:                 True   (0x4000 DllCharacteristics + link /guard:cf)
cet_compat:               True   (EX_DLLCHARACTERISTICS type-20 record bit 0x1)
cf_instrumented:          True   (rustc -C control-flow-guard=checks; GuardFlags 0x100)
cf_function_table_present:True
imports: ADVAPI32, api-ms-win-core-synch, BCRYPTPRIMITIVES, COMBASE,
         KERNEL32, KTMW32, NTDLL, OLE32, OLEAUT32, SHELL32, SHLWAPI,
         USER32 — system DLLs only; no packers, no runtime extraction
Signature: UNSIGNED DEVELOPMENT BUILD → public_release_ready: BLOCKED
```

The packaged-C++ verifier's zip/manifest schema (`--schema static-single`)
still targets the CMake artifact layout; wiring it to the Rust release
directory layout is follow-up work recorded here rather than claimed.
An earlier release attempt (`...-rust-466fd81e`) was superseded because a
RUSTFLAGS environment override silently dropped the config-file hardening
flags; commit fc3eaf6 fixes the script and the hardened rebuild above is
the current artifact.

## Manual acceptance (A01–A14): PASS (operator-confirmed)

The operator verified the produced EXE interactively on the reference
machine and confirmed the full matrix as problem-free on 2026-10-17
("我已经手动验证完全没有问题"). The per-item evidence trail (screenshots /
session transcripts per row) was not captured by this migration run — the
operator's consolidated confirmation is the recorded authority. Automated
tests never launched, stopped, or modified the real Desktop/daemon/Home
beyond read-only AppModel enumeration; `launch --json` invocations during
parity probes were refused by the running-process gate before any
activation.

## Known deviations from the C++ (intentional, reviewed)

1. `build-info` `language` is `Rust` (the one sanctioned schema change).
2. `licenses` lists the Rust dependency notice set (ratatui, crossterm,
   toml, windows-rs, project license) instead of the C++ set.
3. The settle path does not call `CancelSynchronousIo` on reader threads
   (Rust threads do not expose handles); unblocking relies on pipe closure
   after terminating the owned child, inside the same 3 s settle budget.
4. TUI line editor is character-based (no grapheme-cluster cursor moves);
   host/port fields are ASCII-validated by the engine anyway.
5. `Event::Poll` uses a 20 ms crossterm poll slice instead of a blocking
   Win32 wait; busy/timeout semantics are driven by the same session
   budgets.

## Remaining risks / next steps

- A01–A14 confirmed by the operator; the per-row evidence trail (per-item
  captures) can still be backfilled opportunistically.
- Extend `scripts/verify-package.py` to the Rust release layout so the
  packaged artifact gets the same WinVerifyTrust/zip/manifest machine gate
  the C++ releases had (the PE mitigation + import core already passes on
  the Rust EXE).
- The TUI snapshot matrix (U16) remains unautomated; manual acceptance
  covers the interactive surface.
- C++ product retired after this confirmation; the C++ implementation
  remains retrievable from git history (`main@0bdf2ed` and the
  `v0.4.0-rc.1`/`v0.6.1-rc.1` tags).

## 2026-10-27 输入层修复与验收（TUI 代理编辑器）

用户报告"TUI 配置代理一直异常（输入异常、保存异常）"。根因分析确认了五个
产品缺陷并全部修复（`crates/cpg-tui`）：

1. **粘贴注入按键序列**（最严重）：启用了 bracketed paste，但 crossterm
   0.29 在 Windows 上只读 `INPUT_RECORD`、永不合成 `Event::Paste`，于是
   粘贴被拆成 `ESC[200~…ESC[201~` 按键注入——首个 `Esc` 直接关闭编辑器，
   其后字符落入主界面（含 `l` 触发启动、`q` 退出）。修复：不再请求
   bracketed paste（Windows Terminal 此后以普通按键投递粘贴），同时保留
   防御性 `Event::Paste` 处理（过滤控制字符/端口非数字）。
2. **Ctrl/Alt 组合键被当作普通字母**：`Ctrl+L` 会启动 Desktop、`Ctrl+V`
   在编辑器插入字面 `v`。修复：带 Ctrl/Alt 的字符事件统一拦截（Shift 放行）。
3. **光标编辑未实现**（DESIGN.md 承诺 `Home/End/Left/Right/Backspace/Delete`，
   实际全部空操作且注释谎称"委托给 line buffer"）：实现字符级
   `EditorField` 光标编辑并在编辑器渲染反显光标块。
4. **notice 从未渲染**：8 处赋值 0 处读取——"Proxy saved."、"Saved. Status
   refresh failed…"、"Diagnostic copied." 等提示全部不可见。修复：动作栏
   第二行内联显示，任意按键后消隐。
5. **`tui.alternate_screen = "never"` 失效**：`ratatui::init()` 无条件进入
   备用屏且退出不恢复，用户终端滞留备用屏。修复：改用
   `try_init_with_options(Viewport::Fullscreen)`，进出备用屏完全由 guard
   按策略掌控；初始化失败从 panic 改为按文档降级 plain 控制台。
   另修复 plain 控制台启动/变更结果**每条命令后无限复读**（按 operation id
   去重）并清理死代码（no-op `Push/PopKeyboardEnhancementFlags`、恒假
   `modal_editor`、`clone` 即 `drop`）。

保存链路核查结论：engine 侧事务（独占句柄、解析值比对、seek-zero 写入 +
`SetEndOfFile` 截断、失败回滚）健全，非 loopback 主机被拒为产品安全边界；
TUI 同步失败路径不再伪造 `MutationOutcome`，改为内联展示原因并按
`error.field` 聚焦出错字段。

验证：新增 8 个键路/编辑器单元测试 + 2 个 notice 渲染测试（ratatui
`TestBackend`），全套 `scripts/test-rust.ps1` 门禁通过（fmt/clippy
`-D warnings`/99 测试/diff check），并以临时 `--config` 完成 plain 控制台
`E → localhost:10809 → 保存 → 刷新` 端到端真机烟测（未触碰真实 Codex
Home/Desktop/daemon）。

### ConPTY acceptance status（未完成，测试暂 `#[ignore]`）

为覆盖交互面，新建 `app/tests/conpty_tui.rs`：以 Windows 伪终端
（`CreatePseudoConsole` + 属性表子进程）驱动真实 EXE，规划断言粘贴、Ctrl
键惰性、光标编辑、`alternate_screen` 两种策略、保存往返与干净退出。harness
自身曾有一个 bug（`STARTUPINFOEXW.lpAttributeList` 未接线，子进程落到测试
控制台）已修复；此后 conhost 初始化序列（`ESC[?9001h ESC[?1004h`）可以
回流，但**子进程自身输出始终不回流**——连 `cmd.exe /c echo` 都只收到初始
序列，判定为本机 conhost 直通/管线问题而非产品缺陷。三个测试保留为
`#[ignore]`（可 `cargo test -- --ignored` 复跑），待排查 conhost 输出直通
后启用。**下次公开发布前，编辑器相关项（粘贴、Ctrl 键、光标编辑、
`alternate_screen=never`）仍需在 Windows Terminal 中人工复验。**

### Windows key-release regression fix (2026-10-27)

The input dispatcher previously treated `KeyEventKind::Release` as another
keypress. A physical character key inserted twice, and Tab switched to the
port and immediately back. Releasing E after opening the editor also inserted
an unwanted `e` into the host. `Model::key` now ignores releases before any
notice, shortcut, busy-state or modal handling; presses and repeats retain
their existing behavior.

Four new regression tests cover the opening-key release, full press/release
editing sequences, release-only events across every modal, and held-key
typing/cursor/deletion. The existing Tab/BackTab test now exercises complete
press/release pairs. Before the fix, the new tests reproduced `abcdXX`
instead of `abcdX` and `127.0.0.1e` after opening the editor. After the fix,
`scripts/test-rust.ps1` passed fmt, Clippy `-D warnings`, 103 tests and the
diff check. The three existing ConPTY integration tests remain ignored.

A separate smoke run drove the actual debug EXE through the execution host's
Windows pseudo-terminal with a dedicated `target/input-fix-smoke.toml`:
E opened a clean host field; one `x` inserted once and Backspace removed it;
Tab focused the port; five Backspaces followed by `8a080` produced `8080`;
Shift+Tab returned to the host; Enter saved `127.0.0.1:8080`; Q exited with
code 0 and restored the alternate screen. Only the temporary Guard config
was written. No Desktop activation, daemon command or Codex Home write was
requested. This synthesized terminal smoke does not replace physical-keyboard
Windows Terminal acceptance or resolve the ignored integration harness.

The canonical Rust release script also built an unsigned development package
from a clean, isolated source snapshot (the main working tree remains
uncommitted):

```text
Artifact: dist/releases/CodexProxyGuard-0.8.0-rc.1-rust-cddb2776/CodexProxyGuard.exe
Snapshot: cddb2776f51abcbc0aa7b49670652a56c0c49931
SHA256: A959C4DB7786925994A56F781D60EA3079F0379F5165845A80EB124AB51564D1
Size: 1,199,104 bytes
Signature: NotSigned (UNSIGNED DEVELOPMENT BUILD)
```

Release-profile tests passed the same 103 tests. The delivered EXE repeated
the terminal smoke above, saved port `8081` to a second temporary Guard
configuration, and exited with code 0. SHA/build-info matched, changed text
sources matched the snapshot after normal text EOL conversion, frozen golden
fixtures matched byte-for-byte, and all seven PE mitigation facts were true.
The import scan used the existing verifier's parsing core and system-DLL
allowlist plus `BCRYPTPRIMITIVES.DLL` (already present in the earlier Rust
release; its System32 copy has a valid Microsoft Windows signature). This
does not claim that the legacy C++ package schema verifier supports Rust.
The `.cmd` wrapper's Windows PowerShell could not resolve `Get-FileHash` on
this host; rerunning the unchanged canonical `.ps1` with PowerShell 7 completed
the package. No release-script or verifier behavior was changed for this fix.

### Port-chain review follow-up (2026-10-27)

The "edited port does not take effect" review is recorded in
[PROXY_PORT_CHAIN_ACCEPTANCE.md](PROXY_PORT_CHAIN_ACCEPTANCE.md): two
state-machine defects (a failed configuration read reset the displayed endpoint
to the built-in default; `!ready` authorized a recovery rewrite of a path Guard
could not read at all), the launch plan promoted to a typed `SessionView` fact
with early gating for registered targets only, and delivery-versus-adoption
wording in both frontends plus the copied diagnostic. 21 new hermetic tests
bring the workspace to 124 passing, 3 ignored. The real-machine A→B scenario,
Desktop argument adoption and the backend reload checks stay explicitly
**NOT RUN**; the ConPTY harness limitation above is unchanged.
