# Repository Guidance

<!-- CODEGRAPH_START -->
## CodeGraph

This repository is indexed by CodeGraph. Use `codegraph_explore` before grep/read for
cross-crate structure, call paths, and blast-radius analysis. File watching updates the
index automatically; run `codegraph sync .` only when `codegraph status .` reports a
stale or unhealthy index.
<!-- CODEGRAPH_END -->

## Workspace responsibilities

- `proxy-guard-core`: minimal configuration, domain state, reducer, capabilities, and
  redaction; no terminal, network, process, or Windows dependencies.
- `proxy-guard-windows`: bounded APPX discovery, Desktop-root detection, cross-process
  startup locking, environment injection, and process launch.
- `codex-proxy-guard`: minimal CLI, single-screen TUI, dispatch, and launch orchestration.

Preserve the state boundary `Action -> candidate reduce -> authorize -> commit ->
dispatch -> TaskResult`. Only one foreground operation may be active. Guard shutdown
must not terminate Desktop.

## Security invariants

Only loopback HTTP/Mixed proxies are allowed. Never read Token/Cookie/auth files,
decrypt TLS, modify the Windows system proxy, edit `~/.codex/config.toml`, or add
TUN/WFP/WinDivert/hooks/relay behavior. External text and commands must be bounded,
timed out, cancellable where asynchronous, and redacted before display.

Do not add network health probes, Node Readiness, Usage/account telemetry, Codex
app-server or private IPC, v2rayN management, diagnostics/history persistence, or
process termination. The product owns only proxy-environment injection into a newly
launched Desktop process tree.

## Completion commands

```powershell
codegraph status .
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo audit
.\scripts\build-portable.cmd
```

Windows portable artifacts must always come from the canonical script. Completion also
requires release build/package smoke, SHA/build-info verification, diff review, and
documentation synchronization.
