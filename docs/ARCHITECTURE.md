# Architecture

The application is pure C++20 over the Windows SDK and the STL: no Qt, no
Rust, no FFI, no telemetry. The single production executable —
`CodexProxyGuard.exe` — is a Console-subsystem binary built by the root CMake
project with MSVC `/W4 /WX`, the static CRT (`/MT`), and the `/sdl` +
`/guard:cf` hardening applied to **every** compiled object through the shared
`cpg_build_options` INTERFACE target (library code included, so CFG has no
partial-coverage hole), plus the /guard:cf/DYNAMICBASE/NXCOMPAT/
HIGHENTROPYVA/CETCOMPAT link flags. Runtime dependencies are vendored
header-only libraries (toml++ 3.4.0, nlohmann/json 3.12.0); test binaries
additionally use vendored doctest 2.4.12.

## One executable, two process roles

```text
CodexProxyGuard.exe                       interactive TUI (same process)
  └─ CodexProxyGuard.exe
       internal-activate-package          short-lived activation worker
```

The TUI talks to the engine **in-process**: `app/main.cpp` classifies the
Unicode command line (`app/startup_mode.cpp`, pure function, unit-tested),
then either starts the interactive frontend or runs the headless CLI
(`backend/src/cli.cpp`). There is no GUI→bridge self-child anymore; the only
child process Guard ever spawns is its own short-lived activation worker
(`CREATE_NO_WINDOW`, redirected stdio) and, on the explicitly confirmed
repair path, the official Codex CLI running the public
`app-server daemon stop` command.

The engine layering is strict:

- `foundation/`: shared primitives — `publicText` redaction scanner, strict
  UTF-8 conversions, Windows path normalization/containment/canonicalization
  through held handles, strict JSON object parsing (duplicate keys at every
  level via an escape-decoded key walker, depth/size limits, no comments or
  trailing data), the Win32-event `Cancellation`, `UniqueHandle` RAII, the
  `UnicodeEnvironment` block builder (emits the CreateProcessW-required
  ordinal-sorted, duplicate-collapsed double-NUL block) and the bounded
  `runOwnedProcess` runner (separate program/arguments, no shell; the three
  child-side pipe ends are duplicated as the only inheritable handles Guard
  ever holds and passed through a PROC_THREAD_ATTRIBUTE_HANDLE_LIST; stdin is
  written by a dedicated cancellable writer thread that closes the write end
  for EOF; stdout/stderr drain concurrently with per-stream limits, read
  errors are distinguished from EOF, the verdict is re-checked after all
  joins so a fast-exiting flood cannot pass as normal, and post-submission
  failures keep the outcome-unknown semantics).
- `backend/src/core.*`: strict TOML via toml++ 3.4.0, loopback validation,
  config transactions/read leases, marked `.env` transactions, redaction and
  strict UTF-8/JSON.
- `backend/src/platform.*` + `package_discovery.*`: native AppModel package
  discovery (`FindPackagesByPackageFamily`/`PackageIdFromFullName`/
  `GetPackagePathByFullName`/IAppxFactory manifest reader — no PowerShell,
  no fallback to it), process/root and elevation checks, owned helper
  execution, native activation and held-handle package/AUMID/elevation/
  creation/lifetime observations.
- `backend/src/launch.*`: Chromium proxy arguments, process environment,
  safe CLI resolution, explicit public daemon stop, cross-process startup
  lock and the shared launch pipeline.
- `backend/src/session.*`: the transport-free engine session — candidate
  validation, single-use confirmation tokens, the one-foreground-effect
  gate, TaskResult application. It is the single state machine behind every
  interactive frontend; its JSON snapshot is the frontend view model.
- `backend/src/cli.*`: the headless CLI and the `licenses` notice streamer
  (embedded RCDATA resources, no GUI, no configuration, no network).
- `tui/src`: the Win32 VT TUI — `TerminalSession` (RAII console modes, VT
  enable, alternate screen, title/cursor restoration), `ReadConsoleInputW`
  normalization, an off-screen `Screen` cell model with a single-write VT
  renderer, Unicode display width (surrogate pairs, combining marks,
  fullwidth forms), the `TuiState` modal machine (proxy editor, YES-typed
  consent/repair confirmations, error modal with suggested follow-ups and a
  redacted clipboard diagnostic, help, too-small notice) and the plain
  line console fallback (`cli_console.cpp`).
- `app/`: the single production entry, role classification, the embedded
  notice and VERSIONINFO resources.

The engine never links the TUI; the entry point injects the interactive
frontend through a function pointer, keeping `guard_engine` free of any
frontend dependency.

## State and execution

`Action -> candidate validation -> authorization -> commit foreground effect
-> dispatch -> TaskResult` remains the boundary. A session accepts at most
one foreground effect; busy state gates every destructive key except
Ctrl+C and the busy-quit prompt. When an operation exceeds its
`SessionBudgets` budget the session cancels it but stays busy — refusing new
operations — until the effect's future is reaped; only then does it surface
`SESSION_TIMEOUT` (or the effect's own `APPX_ACTIVATION_OUTCOME_UNKNOWN` /
`CODEX_DAEMON_STOP_*` outcome when that is more precise). Nothing is ever
detached or dropped. Completion invalidates stale launch observations;
refresh issues new confirmations. Consent proposals bind a
CSPRNG single-use token to the current config and the exact Home. Mutation
transactions compare expected config under a held Windows handle. A launch
holds an incompatible read lease throughout, preventing stale launches from
recreating an env block after revocation.

The `.env` adapter preserves bytes outside exactly one known marker pair,
rejects conflicting variables/edited blocks and commits a filesystem
transaction after content re-verification. Revocation removes the block
before committing disabled consent. The user Home is resolved explicitly,
never inferred from Guard's `CODEX_HOME`.

## Launch

Every launch revalidates configuration/elevation and freshly discovers
Desktop. A per-user cross-process lock serializes startup and holds a short
post-submission cooldown. Running or uninspectable Desktop roots block
launch. Registered targets require a FullTrust manifest entry and dynamic
AUMID, then use `AO_NONE` through the short-lived activation worker. The
worker needs no package identity. It observes the returned PID via a held
process handle and never owns Desktop's lifetime. Failure after submission
may mean an already-started Desktop; there is no retry or bare-EXE fallback.
Normal launch never resolves the Codex CLI. Repair first prepares an
authorized proxy block, then resolves an absolute CLI and performs at most
one public `app-server daemon stop` (720-second total budget).

Package discovery is a family allowlist (publisher id measured from the
installed package) resolved through native APIs; a stale allowlist yields
zero records and `CODEX_NOT_INSTALLED` (fail closed) because identity is
re-derived from `PackageIdFromFullName` and compared before any launch.

## Verification

doctest suites cover foundation primitives (text/path/json/process),
engine core/platform/launch/CLI, TUI state/width/clipping/session, and the
verifier fixtures (`scripts/tests/test_verify_package.py`). The engine
platform suite includes the native-discovery parity gate against the frozen
PowerShell baseline fixture.
`scripts/test-noqt.ps1` runs every suite;
`scripts/build-portable.cmd` repeats the release tests, executes the
packaged EXE's build-info, scans imports, (optionally) signs and verifies,
and re-verifies every artifact through `scripts/verify-package.py --schema
static-single` before promotion to `dist/releases/<version>-<commit>`.
Release directories are immutable — the build refuses to overwrite an
existing version/commit — and the verifier additionally binds the source
ZIP digest across the sidecar/manifest/SHA256SUMS, rejects build products
inside it, requires the exact artifact set, re-validates signed releases
through WinVerifyTrust, and gates the PE on CFG instrumentation (FID table),
ASLR, NX, high-entropy VA and CET before promotion.
Rollback to the Qt era is the `qt-gui-v0.6.1-rc.1` tag
(see `docs/history/QT_GUI_HISTORY.md`).

## Filesystem transaction decision

A held Windows reader can exclude writers, but allowing an atomic
replacement also allows a different editor to rename-save between
verification and replacement. A second content check does not close that
window. The `.env` adapter therefore uses an isolated NTFS transaction for
expected-content comparison and atomic write/delete commit. Unsupported
filesystems/runtime configurations fail closed; there is no nontransactional
fallback and no change to system policy. If TxF is removed or disabled,
backend block management must be redesigned or refused; ordinary launches
without that optional block remain available.
