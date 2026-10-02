# C++ architecture

The application is C++20 throughout, using Qt 6.8.3 and the Windows SDK. The
Qt Widgets GUI remains isolated from system operations by a versioned stdio
bridge. Both executables are built by the root CMake project with MSVC /W4 /WX.
There is no Rust, FFI, Qt Network, private Codex IPC or telemetry component.

## Modules

- `backend/src/core.cpp`: strict TOML via pinned toml++ 3.4.0, loopback validation,
  config transactions/read leases, marked `.env` transactions, redaction and strict
  UTF-8/JSON (including duplicate-key rejection).
- `backend/src/platform.cpp`: bounded APPX PowerShell discovery, process/root and
  elevation checks, owned helper execution, native activation and held-handle
  package/AUMID/elevation/creation/lifetime observations.
- `backend/src/launch.cpp`: Chromium proxy arguments, process environment, safe CLI
  resolution, explicit public daemon stop, cross-process startup lock and the
  shared launch pipeline.
- `backend/src/bridge.cpp`: request validation, candidate authorization, one
  foreground effect, task-result application, session confirmation tokens and
  bounded NDJSON transport. The owner thread mutates session state; workers receive
  configuration/cancellation snapshots.
- `backend/src/main.cpp`: minimal CLI and line-oriented console. Both use the same
  core transactions and launch pipeline as the bridge.
- `gui/src`: process client, protocol decoder, controller, accessible Qt Widgets.

## State and execution

`Action -> candidate validation -> authorization -> commit foreground effect ->
dispatch -> TaskResult` remains the boundary. A session accepts at most one
foreground effect. Busy snapshot returns cached state, while hello, cancellation
and shutdown remain responsive. Completion invalidates stale launch observations;
refresh issues new confirmations. No operation is replayed after engine failure.

Consent proposals bind a random single-use token to current config and exact Home.
The GUI cannot submit a replacement Home, executable or command line. Mutation
transactions compare expected config under a held Windows handle. A launch holds
an incompatible read lease throughout, preventing stale launches from recreating
an env block after revocation. Configuration writes keep that same handle;
restoration is attempted on write failure, but physical storage failure is not a
power-loss transaction guarantee.

The `.env` adapter preserves bytes outside exactly one known marker pair, rejects
conflicting variables/edited blocks and atomically replaces after content
re-verification. Revocation removes the block before committing disabled consent.
The user Home is resolved explicitly, never inferred from Guard's CODEX_HOME.

## Launch

Every launch revalidates configuration/elevation and freshly discovers Desktop.
A per-user cross-process lock serializes startup and holds a short post-submission
cooldown. Running or uninspectable Desktop roots block launch. Registered targets
require a FullTrust manifest entry and dynamic AUMID, then use AO_NONE through a
short-lived `internal-activate-package` worker in the engine executable.
The worker needs no package identity. It observes the returned PID via a held
process handle and never owns Desktop's lifetime. Failure after submission may
mean an already-started Desktop; there is no retry or bare-EXE fallback.

Normal launch never resolves Codex CLI. Repair first prepares an authorized proxy
block, then resolves an absolute CLI and performs at most one public
`app-server daemon stop` (720-second total budget). It rechecks registration,
processes and cancellation before activation. A failed launch after a confirmed
stop explicitly reports the shared-daemon side effect. There are no daemon
start/restart/update/bootstrap paths.

The embedded `resources/appx-discovery.ps1` emits schema 1 JSON with explicit
serialization depth and optional null manifest attributes. C++ strictly validates
records, executable containment and target identity. Helper outputs/time are
bounded; cancellation kills and reaps only direct Guard-owned helpers.

## Verification

Backend QtTest suites cover configuration/env transactions and leases, strict
protocol and discovery fixtures, worker rejection, helper timeout/cancellation,
CLI resolution, launch ordering and failed preparation, plus real bridge pipe
shutdown. Existing GUI protocol/controller/process tests cover frontend lifecycle.
`test-cpp.ps1` runs all suites. `build-portable.cmd` repeats release tests and
checks portable runtime loading, source provenance and checksums.

The version-2 config schema survives migration for data safety. The old full-screen
Rust TUI is replaced by the console; no source/binary fallback to Rust remains.
