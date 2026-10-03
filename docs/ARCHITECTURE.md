# C++ architecture

The application is C++20 throughout, using Qt 6.8.3 and the Windows SDK. The
Qt Widgets GUI remains isolated from system operations by a versioned stdio
bridge. There is exactly one production executable — `CodexProxyGuard.exe` —
which selects its process role from structured argument classification before
any Application object exists. It is built by the root CMake project with MSVC
/W4 /WX and links the pinned static Qt SDK with the static CRT (/MT) for the
release profile. There is no Rust, FFI, Qt Network, private Codex IPC or
telemetry component.

## One executable, several process roles

```text
CodexProxyGuard.exe                       GUI process: QApplication
  └─ CodexProxyGuard.exe bridge           engine process: QCoreApplication
       └─ CodexProxyGuard.exe
            internal-activate-package     short-lived activation worker
```

`app/main.cpp` is the single production entry point. It reads the Unicode
command line (`GetCommandLineW`/`CommandLineToArgvW`), classifies it with
`app/startup_mode.cpp` (pure function, unit-tested in `app/tests`), and only
then constructs exactly one application object: `QApplication` for the GUI role
(no arguments, `--config` without a command, `--smoke-test`) or
`QCoreApplication` for headless roles (`bridge`, `internal-activate-package`,
`build-info`/`--build-info`, `launch`, `init-config`, `config-path`,
`console`, `licenses`, `--help`, `--version`). Contradictory or unknown
argument combinations exit nonzero before any Application exists. Option
values are consumed as values, so a `--config` path containing `bridge` or
`--smoke-test` cannot flip the role.

The GUI relaunches its own file (`QCoreApplication::applicationFilePath()`)
with the `bridge` argument; the activation worker is spawned the same way
inside `platform.cpp`. Test seams inject a fake engine path through a
dedicated constructor only — production never overrides the relaunch target.
The same binary hosting several roles does not grant the GUI any direct
system-operation channel: `guard_gui` still links no engine business code; only
the final `app` aggregation target links both libraries.

The Windows GUI subsystem is kept (no console flash on double-click) while
headless roles use the inherited standard handles through bounded Win32 I/O;
the bridge protocol itself is unchanged (NDJSON schema 1, 128 KiB frames).
Headless roles never initialize a QPA platform — a bogus `QT_QPA_PLATFORM`
does not affect `build-info`, the bridge or worker rejection. The `licenses`
command streams embedded notice resources over Win32 stdout outside the frame
protocol.

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
- `backend/src/cli.cpp`: minimal CLI and line-oriented console (`runCli`) plus the
  `licenses` notice streamer. CLI, console and bridge share the same core
  transactions and launch pipeline.
- `gui/src`: process client, protocol decoder, controller, accessible Qt Widgets,
  the offline licenses viewer (a short-lived same-EXE `licenses` child with bounded
  output and deadlines) and `gui_entry.cpp` (`runGui`) performing GUI initialization
  for the QApplication the entry point constructed.
- `app/`: the single production entry, role classification and the embedded
  notice resources.

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
conflicting variables/edited blocks and commits a filesystem transaction after content
re-verification. The transaction isolates both writers and editor rename-save races. Revocation removes the block before committing disabled consent.
The user Home is resolved explicitly, never inferred from Guard's CODEX_HOME.

## Launch

Every launch revalidates configuration/elevation and freshly discovers Desktop.
A per-user cross-process lock serializes startup and holds a short post-submission
cooldown. Running or uninspectable Desktop roots block launch. Registered targets
require a FullTrust manifest entry and dynamic AUMID, then use AO_NONE through a
short-lived `internal-activate-package` worker spawned from the same executable.
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
bounded; cancellation kills and reaps only direct Guard-owned helpers. Processes
are always managed through held handles of Guard's own children — never by
executable name, which matters now that GUI, engine and worker share one EXE.

## Static linking and plugins

The release profile (`CPG_STATIC_QT`) links the pinned static Qt 6.8.3 SDK
(built by `scripts/build-qt-static.ps1` with `-release -static -static-runtime`)
and the static CRT for every target; a dynamic SDK mixed with /MT fails
configuration via imported-target type checks. `QT_STATIC` itself is only ever
defined by Qt's static package files. The production executable imports an
explicitly verified static plugin set — Windows QPA integration, gif/ico/jpeg
image codecs and the modern Windows style — through `qt_import_plugins(...,
NO_DEFAULT)`. Test executables import only the offscreen integration they run
under (`QT_QPA_PLATFORM=offscreen`); production never links offscreen. The
release import-table gate (dumpbin /DEPENDENTS) rejects any Qt6*, msvcp140*,
vcruntime140*, concrt140* or unexpected third-party DLL import; normal Windows
system DLLs and API sets are the only accepted dependencies.

Notices are embedded as resources (project MIT license, third-party notices and
the complete Qt license text set) and streamed by the `licenses` command without
GUI, configuration or network access. The GUI's Help → About "Licenses /
Third-party notices" viewer displays the same stream offline by spawning this
executable's `licenses` role once (never the bridge protocol) and decoding the
complete bounded byte buffer in one pass; closing the window aborts and reaps
only that Guard-owned child. Corresponding source ships in the separate
source-compliance archive described by `THIRD_PARTY_NOTICES.md`; replacing Qt in
a static build means rebuilding the static SDK per `docs/STATIC_QT_REBUILD.md`.

## Verification

Backend QtTest suites cover configuration/env transactions and leases, strict
protocol and discovery fixtures, worker rejection, helper timeout/cancellation,
CLI resolution, launch ordering and failed preparation, plus real bridge pipe
shutdown. Existing GUI protocol/controller/process tests cover frontend lifecycle,
and `app_startup_mode` covers role classification (including values containing
role words, separators and contradictory modes). `test-cpp.ps1` runs all suites
against a dynamic SDK, or with `-StaticQt` against the pinned static prefix.
`build-portable.cmd -StaticQt` repeats release tests, checks the single-file
runtime in an isolated directory with a System32-only PATH, executes the packaged
executable's embedded build-info, scans imports, and re-verifies every artifact
through `scripts/verify-package.py --schema static-single` before promotion to
`dist/releases/<version>-<commit>`.

The version-2 config schema survives migration for data safety. The old full-screen
Rust TUI is replaced by the console; no source/binary fallback to Rust remains.

## Filesystem transaction decision

A held Windows reader can exclude writers, but allowing an atomic replacement also
allows a different editor to rename-save between verification and replacement.
A second content check does not close that window. The `.env` adapter therefore
uses an isolated NTFS transaction for expected-content comparison and atomic
write/delete commit. Unsupported filesystems/runtime configurations fail closed;
there is no nontransactional fallback and no change to system policy.

This is a deliberate narrow dependency, not a claim that TxF is generally the
preferred Windows storage API. [Microsoft recommends alternatives and warns that
TxF may be unavailable in future Windows versions](https://learn.microsoft.com/en-us/windows/win32/fileio/transactional-ntfs-portal).
The stricter requirement here is to preserve an independently edited `.env` outside
Guard's block, including concurrent rename-save, while committing one visible
change. If TxF is removed or disabled, backend block management must be redesigned
or refused; ordinary launches without that optional block remain available.
