# Repository Guidance

<!-- CODEGRAPH_START -->
## CodeGraph

This repository is indexed by CodeGraph. Use `codegraph_explore` before grep/read for
cross-crate structure, call paths, and blast-radius analysis. File watching updates the
index automatically; run `codegraph sync .` only when `codegraph status .` reports
a stale or unhealthy index.
<!-- CODEGRAPH_END -->

## Workspace responsibilities

Pure Rust 2024 workspace (`resolver = "3"`, toolchain pinned by
`rust-toolchain.toml`); no Qt, no C/C++ product code, no Node/Bun/Python
runtime dependency in the product, no PowerShell for package discovery.

- `crates/cpg-domain/`: pure contracts, no I/O, no Win32 — config schema v2
  with byte-exact canonical serialization, strict JSON (duplicate keys at
  every level, depth/size limits), `publicText` redaction, loopback proxy
  validation with InetPton semantics, the dotenv managed-block lexical
  gate and splice logic, AppModel discovery selection rules and the
  activation-worker protocol validation.
- `crates/cpg-windows/`: the single auditable Win32 boundary; every
  `unsafe` call lives here behind safe wrappers with `SAFETY:` notes —
  RAII handles, event cancellation, ordinal-sorted Unicode environment
  blocks, canonical path/final-path queries, the bounded `runOwnedProcess`
  runner (restricted handle inheritance, concurrent drains, cancellable
  stdin writer, hard settle before verdict), TxF transactions, console
  control and mode guards, token elevation, Toolhelp process state,
  clipboard, native AppModel discovery and the activation observer.
- `crates/cpg-engine/`: business orchestration — held config transactions
  and read leases, consented atomic `.env` prepare/revoke (TxF with
  expected-bytes re-read), the launch pipeline (proxy plan, CLI
  resolution, exactly-one daemon stop after preparation, startup lock,
  receipt classification) and the `EngineSession` state machine (single
  foreground gate, single-use revision-bound tokens,
  `CancellingAfterTimeout` until reaped, receipts preserved over refresh
  errors).
- `crates/cpg-tui/`: Ratatui/Crossterm console and the plain line console.
  The TUI never recomputes engine business rules; it renders the typed
  `SessionView` and forwards intents to the session.
- `app/`: the single production entry `CodexProxyGuard.exe` — strict CLI
  parsing (C++-parity usage text), the hidden `internal-activate-package`
  activation worker (same EXE, bounded stdio framing), embedded build
  provenance and the offline license stream.

Dependency direction is fixed: `app` → `cpg-tui`/`cpg-engine` →
`cpg-domain` ← `cpg-windows`; `cpg-domain` imports nothing platform- or
terminal-specific, and no crate outside `cpg-windows` performs Win32
calls. Internal contracts are typed Rust enums/structs; JSON is an
external adapter for the CLI and worker protocol only.

Every crate builds with the shared hardening in `.cargo/config.toml`
(`-C control-flow-guard=checks` plus `/CETCOMPAT /guard:cf /DYNAMICBASE
/NXCOMPAT /HIGHENTROPYVA` link args). The release script re-sets the full
RUSTFLAGS set explicitly because an environment RUSTFLAGS replaces config
rustflags (commit fc3eaf6). Keep `cargo clippy --workspace --all-targets
-- -D warnings` clean. Preserve the state boundary `Action → candidate
validation → authorize → commit → dispatch → TaskResult`. Only one
foreground operation may be active — including while a timed-out
operation is being reaped. Guard shutdown must not terminate Desktop.
Guard reaps only its own children through held process handles — never by
executable name.

## Security invariants

Production must not contain Qt or C/C++ product code and must not launch
powershell.exe/pwsh.exe for package discovery; there is no PowerShell
fallback. The TUI does not duplicate engine business judgment, and the
default no-argument invocation is the in-process TUI (the activation
worker and the explicitly confirmed Codex CLI helper are the only child
processes).

Only loopback HTTP/Mixed proxies are allowed. Never read Token/Cookie/auth
files, decrypt TLS, modify the Windows system proxy, edit
`~/.codex/config.toml`, or add TUN/WFP/WinDivert/hooks/relay behavior.
External text and commands must be bounded, timed out, cancellable where
asynchronous, and redacted before display.

The single `~/.codex/.env` exception: Guard may manage exactly one
BEGIN/END-marked `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` block inside one
explicitly authorized Codex Home (`codex.manage_codex_proxy_env` bound to
an absolute `codex.proxy_env_home`; default off; consent granted or
revoked only through an explicit single-use prompt). The file is only
auto-edited when every physical line is an independent single-line dotenv
construct: multi-line quoted values, continuations and unterminated
quotes are refused whole-file with `BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED`
— a marker inside a quoted value is value content, never a managed block.
Outside that block nothing is written: existing keys (including
`export KEY=...` dotenv spellings) are reported as conflicts, other bytes
are preserved verbatim, edits are atomic (TxF) with content
re-verification, and the scope is never inferred from Guard's own
`CODEX_HOME`. Revocation removes only Guard's own unmodified block, and
the revoked consent is persisted only after the block is safely gone — a
failed revoke leaves the consent, the bound Home, and the file untouched
(retryable), so an orphan block Guard can no longer locate is impossible.
The block affects later Codex processes sharing that Home and is a file
fact (`backend_proxy_config_prepared`), never a network verification.
`~/.codex/config.toml` remains untouched in all cases.

Normal launches must have zero daemon side effects: they never resolve the
Codex CLI and never run daemon commands. Codex daemon compatibility is
limited to the explicitly authorized repair launch invoking the public
`codex app-server daemon stop` lifecycle command once (single-use user
confirmation per launch; the stop may interrupt shared tasks of other
clients on the same Codex Home). For registered targets the repair launch
additionally requires the authorized backend proxy block
(`BACKEND_PROXY_REQUIRED_FOR_REPAIR` without it), and the block is always
prepared before the daemon stop so a failed preparation never leaves the
shared daemon interrupted. Guard must never connect to the app-server
socket, read daemon private state, inspect auth data,
start/restart/update/bootstrap the daemon, or directly terminate shared
Codex or Desktop processes. Guard may terminate and reap only its own
short-lived helper children (bounded output, bounded wait) on timeout or
cancellation. Guard must refuse to launch when it is itself running
elevated — or when the elevation query itself fails — matching the Codex
background-server elevation requirement.

Do not add network health probes, Node Readiness, Usage/account telemetry,
Codex app-server or private IPC, v2rayN management, sing-box management,
network benchmarks, subscription management, diagnostics/history
persistence, daemon start/restart/update/bootstrap flows, or a persisted
auto-stop configuration. The product owns only proxy delivery into a newly
launched Desktop process tree, split into verified activation arguments
and the consented Home block above.

Registered Desktop applications launch through the Windows application
model: `IApplicationActivationManager::ActivateApplication` with `AO_NONE`
on a dynamically resolved AUMID, executed by a short-lived Guard-owned
worker (the same EXE, hidden subcommand) over one bounded stdin request
and one bounded stdout receipt. The worker needs no package identity,
never runs in the OpenAI package context, and is never terminated together
with the Desktop. A registered target must never be spawned as a bare EXE,
never launched through `Invoke-CommandInDesktopPackage` or
`IPackageDebugSettings`, and never given `--no-sandbox`/debug ports as an
"identity fix". Cancellation before submission guarantees no activation;
after submission the outcome is reported unknown and never auto-retried.
Guard may query the target's package identity, AUMID, elevation, and
creation time through held process handles to verify what was actually
activated. Normal launch still never resolves the Codex CLI or stops the
shared daemon.

Public releases must carry a valid Authenticode signature unless
explicitly built and marked as an unsigned development build. Never add
packers, UPX, obfuscators, self-extracting shells, runtime DLL extraction
or a hand-written PE loader; never attempt to evade antivirus detection in
code.

## Completion commands

```powershell
codegraph status .
.\scripts\test-rust.ps1                  # fmt --check + clippy -D warnings + all tests + diff check
git diff --check
.\scripts\build-portable-rust.cmd -AllowUnsignedDev       # unsigned dev release (clean tree)
.\scripts\build-portable-rust.cmd -Sign -CertificateSha1 <thumbprint>   # public release
```

Windows portable artifacts must always come from the canonical script,
which gates the PE mitigation set with the `verify-package.py` parsing
core (CFG/CET/ASLR/DEP + import scan; wiring its zip/manifest schema to
the Rust release layout is tracked follow-up work). Release directories
are immutable — the build refuses an existing destination and promotes
with one atomic move from unique staging. Completion also requires
release build/package smoke, SHA/build-info verification, import-table
scan, diff review, and documentation synchronization. Automated tests
must never touch a real Codex Home, start the real Desktop or stop a
shared daemon.

## Historical implementations

The C++ (0.7.x) and Qt (0.6.x) products were retired after the Rust
rewrite passed real-machine acceptance (2026-10-17); retrieve them from
git history (`main@0bdf2ed`, tag `v0.6.1-rc.1`) when audit needs arise.
Frozen C++ contract goldens live on in `tests/golden/` and pin the Rust
rewrite's byte-level parity; they are kept byte-exact through
`git archive` via `.gitattributes` — do not re-encode them.
