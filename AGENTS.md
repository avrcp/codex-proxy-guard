# Repository Guidance

<!-- CODEGRAPH_START -->
## CodeGraph

This repository is indexed by CodeGraph. Use `codegraph_explore` before grep/read for
cross-crate structure, call paths, and blast-radius analysis. File watching updates the
index automatically; run `codegraph sync .` only when `codegraph status .` reports a
stale or unhealthy index.
<!-- CODEGRAPH_END -->

## Workspace responsibilities

- `backend/src/core.*`: strict configuration/domain validation, redaction,
  held config transactions/leases and scoped `.env` operations.
- `backend/src/platform.*`: bounded APPX discovery, process/elevation checks,
  owned helpers and native application-model activation/identity observation.
- `backend/src/launch.*`: proxy plans/environment, startup locking, public daemon
  compatibility and shared launch orchestration.
- `backend/src/bridge.*` and `main.cpp`: foreground state boundary, versioned
  stdio bridge, minimal CLI and interactive console.
- `gui/`: Qt Widgets, controller and child-process client; no system-operation
  duplication. All production code is C++20; no Rust/FFI fallback.

Preserve the state boundary `Action -> candidate reduce -> authorize -> commit ->
dispatch -> TaskResult`. Only one foreground operation may be active. Guard shutdown
must not terminate Desktop.

## Security invariants

Only loopback HTTP/Mixed proxies are allowed. Never read Token/Cookie/auth files,
decrypt TLS, modify the Windows system proxy, edit `~/.codex/config.toml`, or add
TUN/WFP/WinDivert/hooks/relay behavior. External text and commands must be
bounded, timed out, cancellable where asynchronous, and redacted before display.

The single `~/.codex/.env` exception: Guard may manage exactly one
BEGIN/END-marked `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` block inside one
explicitly authorized Codex Home (`codex.manage_codex_proxy_env` bound to an
absolute `codex.proxy_env_home`; default off; consent granted or revoked only
through an explicit single-use prompt). The file is only auto-edited when every
physical line is an independent single-line dotenv construct: multi-line quoted
values, continuations and unterminated quotes are refused whole-file with
`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED` — a marker inside a quoted value is value
content, never a managed block. Outside that block nothing is written:
existing keys (including `export KEY=...` dotenv spellings) are reported as
conflicts, other bytes are preserved verbatim, edits are atomic with content
re-verification, and the scope is never inferred from Guard's own
`CODEX_HOME`. Revocation removes only Guard's own unmodified block, and the
revoked consent is persisted only after the block is safely gone — a failed
revoke leaves the consent, the bound Home, and the file untouched (retryable),
so an orphan block Guard can no longer locate is impossible. The block affects later Codex processes sharing that Home and is
a file fact (`backend_proxy_config_prepared`), never a network verification.
`~/.codex/config.toml` remains untouched in all cases.

Normal launches must have zero daemon side effects: they never resolve the Codex
CLI and never run daemon commands. Codex daemon compatibility is limited to the
explicitly authorized repair launch invoking the public
`codex app-server daemon stop` lifecycle command once (single-use user
confirmation per launch; the stop may interrupt shared tasks of other clients on
the same Codex Home). For registered targets the repair launch additionally
requires the authorized backend proxy block (`BACKEND_PROXY_REQUIRED_FOR_REPAIR`
without it), and the block is always prepared before the daemon stop so a failed
preparation never leaves the shared daemon interrupted. The Guard must never connect to the app-server socket, read
daemon private state, inspect auth data, start/restart/update/bootstrap the
daemon, or directly terminate shared Codex or Desktop processes. Guard may
terminate and reap only its own short-lived helper children (bounded output,
bounded wait) on timeout or cancellation. Guard must refuse to launch when it is
itself running elevated — or when the elevation query itself fails — matching the
Codex background-server elevation requirement.

Do not add network health probes, Node Readiness, Usage/account telemetry, Codex
app-server or private IPC, v2rayN management, sing-box management, network
benchmarks, subscription management, diagnostics/history persistence, daemon
start/restart/update/bootstrap flows, or a persisted auto-stop configuration.
The product owns only proxy delivery into a newly launched Desktop process
tree, split into verified activation arguments and the consented Home block
above.

Registered Desktop applications launch through the Windows application model:
`IApplicationActivationManager::ActivateApplication` with `AO_NONE` on a
dynamically resolved AUMID, executed by a short-lived Guard-owned worker (the
same EXE, hidden subcommand) over one bounded stdin request and one bounded
stdout receipt. The worker needs no package identity, never runs in the OpenAI
package context, and is never terminated together with the Desktop. A
registered target must never be spawned as a bare EXE, never launched through
`Invoke-CommandInDesktopPackage` or `IPackageDebugSettings`, and never given
`--no-sandbox`/debug ports as an "identity fix". Cancellation before submission
guarantees no activation; after submission the outcome is reported unknown and
never auto-retried. Guard may query the target's package identity, AUMID,
elevation, and creation time through held process handles to verify what was
actually activated. Normal launch still never resolves the Codex CLI or stops
the shared daemon.

## Completion commands

```powershell
codegraph status .
.\scripts\test-cpp.ps1
git diff --check
.\scripts\build-portable.cmd
```

Windows portable artifacts must always come from the canonical script. Completion also
requires release build/package smoke, SHA/build-info verification, diff review, and
documentation synchronization.
