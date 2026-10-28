# Architecture

The application is pure Rust 2024 over the `windows` crate bindings: no
Qt, no C/C++ product code, no FFI bridges, no telemetry. The single
production executable — `CodexProxyGuard.exe` — is a Console-subsystem
binary built by the Cargo workspace with the toolchain pinned in
`rust-toolchain.toml`. Hardening is applied to **every** crate through
`.cargo/config.toml` (`-C control-flow-guard=checks` codegen plus
`/CETCOMPAT /guard:cf /DYNAMICBASE /NXCOMPAT /HIGHENTROPYVA` link args);
the release script re-sets the identical set through the environment
because an env RUSTFLAGS replaces config rustflags. The static CRT is a
release-build selection (`+crt-static`). Runtime dependencies are the
`windows`/`windows-strings` bindings, `ratatui` + `crossterm` for the
frontend, and `toml` for configuration parsing; the binary imports only
Windows system DLLs.

## One executable, two process roles

```text
CodexProxyGuard.exe                       interactive TUI (same process)
  └─ CodexProxyGuard.exe
       internal-activate-package          short-lived activation worker
```

The TUI talks to the engine **in-process**: `app/src/main.rs` receives
Unicode arguments, and `app/src/cli.rs` parses them exactly once with the
strict parser (single-use value options, exact usage text). The dispatcher
resolves the default or explicit configuration path once to an absolute
path; information commands and workers never load configuration. The only
child processes Guard ever spawns are its own short-lived activation
worker (`CREATE_NO_WINDOW`, redirected stdio) and, on the explicitly
confirmed repair path, the official Codex CLI running the public
`app-server daemon stop` command.

## Crate layering

```text
                     app (strict CLI + role dispatcher)
                       │
             ┌─────────┴─────────┐
             ▼                   ▼
         cpg-tui             cpg-engine
             │                   │
             └─────► cpg-domain ◄┘
                                 ▲
                                 │
                            cpg-windows
```

- `crates/cpg-domain`: pure contracts, zero I/O. `config.rs` holds schema
  v2 plus the byte-exact canonical serializer the product writes
  (alphabetical tables/keys, literal `'…'` strings, `[ 'a', 'b' ]`
  arrays — parity pinned by `tests/golden/`). `json.rs` is the strict
  JSON parser/serializer (escape-decoded duplicate-key rejection at every
  level, depth ≤ 64, integer shape preservation, compact dump). `text.rs`
  is the `publicText` redaction scanner (USERPROFILE masking, ANSI/OSC
  removal, URI credentials, secret lines, control/format filtering).
  `proxy.rs` implements strict port parsing and loopback classification
  with InetPton semantics; `dotenv.rs` the managed-block lexical gate,
  generated-value validation and splice computation; `discovery.rs` the
  AppModel record-selection rules behind an injected `FsProbe`;
  `worker.rs` the activation request/receipt protocol validation.
- `crates/cpg-windows`: the only crate with `unsafe`. `handles`
  (RAII OwnedHandle), `cancellation` (manual-reset event + atomic flag),
  `fsio` (bounded whole-file read/seek-zero write), `paths` (known
  folders, cleanPath normalization with case folding, final-path
  canonicalization through held handles, containment), `environment`
  (ordinal-sorted duplicate-collapsed CreateProcessW blocks),
  `process` (the bounded owned-process runner: three inheritable
  duplicates through a PROC_THREAD_ATTRIBUTE_HANDLE_LIST, concurrent
  stdout/stderr drains with per-stream limits, a cancellable stdin writer
  thread, hard settle of child and threads before any verdict, the full
  HELPER_* failure taxonomy and activation outcome-unknown semantics),
  `txf` (NTFS transactions with commit/rollback proof),
  `console` (control-handler latch, mode guards, stdin framing probe,
  run_console_operation), `elevation` (TokenElevation gates),
  `process_state` (Toolhelp snapshot with the Electron-helper rule),
  `clipboard` (CF_UNICODETEXT), `appmodel` (native discovery:
  FindPackagesByPackageFamily → PackageIdFromFullName →
  GetPackagePathByFullName → IAppxFactory manifest reader) and
  `activation` (IApplicationActivationManager + held-handle identity
  observation).
- `crates/cpg-engine`: `config_store.rs` — held write transactions with
  expected-content gates and consent-evidence preservation, the launch
  read lease, `absoluteHome`/`boundHome` scoping, TxF-guarded `.env`
  prepare/revoke with expected-bytes re-read inside the transaction, the
  consent enable/revoke ordering (revoke removes the block before
  consent is persisted; failures stay retryable) and the rule that a repair
  write never re-creates a missing file with backend consent carried over
  from a stale snapshot. `launch.rs` — proxy plan (Chromium bypass list),
  Unicode proxy environment, safe CLI resolution (absolute .exe override,
  explicit CODEX_HOME, known layouts, PATH absolute dirs), exactly-one
  public daemon stop after preparation, the cross-process startup lock
  exposed as the injectable `mark_submitted` seam, receipt classification
  and the shared `launch_with` orchestration used by tests and production
  alike. Checks that need no file, handle or process run in `preflight`,
  before the lease and the startup lock, so a deterministic refusal leaves
  no trace; the bypass-expressibility refusal stays after discovery,
  because only a discovered target says whether activation arguments are
  required at all.
  `session.rs` — `EngineSession`: the transport-free state machine
  (typed `SessionView`, single-use CSPRNG confirmation tokens bound to
  revision + Home + confirmed config, the one-foreground-effect gate that
  stays busy through `CancellingAfterTimeout` until the worker is reaped,
  committed-mutation facts that survive refresh failures, and the
  configuration-source, coverage, launch-plan and delivery facts —
  configured endpoint, submitted endpoint, divergence — that the frontends
  render instead of recomputing; the endpoint itself is published only for a
  configuration that was read, committed or retained — `ProxyView` carries that
  as `endpoint_known` so a frontend cannot re-derive it from a label — and
  coverage and
  authorization only for a configuration that was actually read, so a session
  with no successful read shows no endpoint and claims no consent.
- `crates/cpg-tui`: `terminal.rs` (capability probe, raw-mode/alternate
  screen guard with panic-path restore, console-title capture and restore),
  `app.rs` (Ratatui event loop,
  modal machine — proxy editor, YES-typed consent/repair, result/help/
  quit-while-busy — key translation, redacted clipboard diagnostic) and
  `plain.rs` (the line console fallback, with its own bounded line reader).
  The frontends never decide
  authorization; they render `SessionView` and forward intents.
- `app`: `cli.rs` (strict parser + dispatcher + launch), `worker.rs`
  (bounded stdin framing, schema-v1 validation, COM activation, receipt
  self-validation), `build_info.rs` (compile-time provenance via
  `option_env!` — absent values are honestly unknown/dirty), `licenses.rs`
  (embedded offline notice stream).

## State and execution

`Action → candidate validation → authorization → commit foreground
effect → dispatch → TaskResult` remains the boundary. A session accepts at
most one foreground effect; busy state gates every destructive key except
Ctrl+C and the busy-quit prompt. When an operation exceeds its
`SessionBudgets` budget the session cancels it but stays busy — refusing
new operations — until the effect's worker is reaped. A budget overrun is
retained alongside a known receipt or committed configuration, never
substituted for those facts. Mutation and refresh errors are separate: a
successful commit followed by a read failure is not retried as a write.
Launch results retain PID, instance, activation, backend block and daemon
facts; the frontend schedules at most one read-only refresh after
settlement and keeps the receipt. A launch that produced no receipt is
described as "no delivery receipt recorded", which is also the honest wording
for an activation whose outcome is unknown — the absence of a receipt is never
presented as proof that nothing was delivered. Consent proposals bind a CSPRNG
single-use token, target enable value and revision to the current config
and the exact Home; any refused or failed submission consumes the
displayed proposal. Mutation transactions compare expected config under a
held Windows handle; a launch holds an incompatible read lease
throughout, preventing stale launches from recreating an env block after
revocation.

## Launch

Every launch revalidates configuration/elevation and freshly discovers
Desktop. A per-user cross-process lock serializes startup (5-second
cooldown) and is marked at the submission boundary. Running or
uninspectable Desktop roots block launch. Registered targets require a
FullTrust manifest entry and dynamic AUMID, then use `AO_NONE` through
the short-lived activation worker. The worker needs no package identity;
it observes the returned PID via a held process handle and never owns
Desktop's lifetime. Failure after submission may mean an already-started
Desktop; there is no retry or bare-EXE fallback. Normal launch never
resolves the Codex CLI. Repair first prepares an authorized proxy block,
then resolves an absolute CLI and performs at most one public
`app-server daemon stop` (720-second total budget).

Package discovery is a family allowlist (publisher id measured from the
installed package) resolved through native APIs; a stale allowlist yields
zero records and `CODEX_NOT_INSTALLED` (fail closed) because identity is
re-derived from `PackageIdFromFullName` and compared before any launch.

## Verification

`cargo test --workspace` runs 89 tests across the four crates: domain
contracts (including byte-exact golden roundtrips against the frozen C++
fixtures in `tests/golden/`), windows adapter (real-machine TxF
commit/rollback, owned-process budgets, native package reads),
engine/session (busy gate, single-use tokens, exactly-once daemon stop,
timeout reap) and CLI parsing parity. `scripts/test-rust.ps1` adds
`cargo fmt --check`, `clippy -D warnings` and `git diff --check`.
`scripts/build-portable-rust.ps1` builds from a pinned `git archive`
snapshot in unique staging directories with the full hardening RUSTFLAGS,
runs the release-mode tests, records provenance and SHA-256, optionally
signs, and promotes immutably to `dist/releases/<version>-rust-<commit>`.
The PE mitigation and import gates use the `verify-package.py` parsing
core; its zip/manifest schema still targets the retired C++ layout and is
tracked follow-up work. The full migration record — including which
matrix rows are machine-verified vs operator-confirmed — is
`docs/RUST_TUI_MIGRATION_ACCEPTANCE.md`.

## Filesystem transaction decision

A held Windows reader can exclude writers, but allowing an atomic
replacement also allows a different editor to rename-save between
verification and replacement. A second content check does not close that
window. The `.env` adapter therefore uses an isolated NTFS transaction
for expected-content comparison and atomic write/delete commit.
Unsupported filesystems/runtime configurations fail closed
(`BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE`); there is no nontransactional
fallback and no change to system policy. If TxF is removed or disabled,
backend block management must be redesigned or refused; ordinary launches
without that optional block remain available.
