# C++ engine migration

Baseline: `63e0ffc` / 0.5.0-rc.1. Target: 0.6.0-rc.1.

## Assessment

Rust currently owns configuration validation and locking, scoped `.env` transactions,
APPX discovery, process/elevation checks, native activation and identity observation,
public daemon-stop compatibility, orchestration, console UI/CLI and the GUI bridge.
Replacing only the executable wrapper would leave the dual toolchain intact.

The migration keeps the GUI and process isolation, implements these responsibilities
in C++20 with Qt Core plus the Windows SDK, and removes Rust sources/toolchain from
the final tree. Qt Network, private Codex IPC and backend FFI remain excluded.
The earlier brief's “keep Rust” constraint is superseded by the user's explicit
request for a complete C++ replacement; the security invariants remain mandatory.

Configuration TOML schema and existing consent/Home bindings are deliberately retained:
discarding them could strand already-authorized proxy blocks. This is a data-safety
decision, not a requirement to emulate Rust internals. Use a pinned, licensed TOML
parser, not a handwritten partial parser. Existing Qt stdio contract remains the
frontend integration boundary. The native activation worker remains a separate
short-lived invocation of the engine executable.

## Stages and commits

1. Record assessment, interfaces and security/parity matrix; retain Rust as reference.
2. Implement/test C++ configuration, `.env` transactions and Windows config leases.
3. Implement/test discovery, process/elevation checks, bounded helpers and activation.
4. Implement/test daemon compatibility, launch orchestration and versioned bridge/CLI.
5. Switch CMake and canonical packaging to C++ only; remove Rust and synchronize docs.
6. Run Windows tests and release/package smoke, independently review failure paths,
   commit/push the completed stages, then build and verify clean-commit packages.

Independent modules may be implemented in parallel against reviewed interfaces.
All production paths must share the same implementation; no fallback to Rust.

## Mandatory parity gates

- Loopback-only proxy and strict config schema; invalid configuration never launches.
- Consent is explicit, single-use and bound to the exact displayed Home/configuration.
- `.env` bytes outside Guard's block survive; conflicts and edited blocks fail closed.
- Held config transactions and launch leases prevent revoke/prepare races.
- Registered targets use AO_NONE application activation with held-handle identity
  checks; never bare EXE, debug identity workaround or automatic retry.
- Normal launch never resolves Codex CLI or invokes daemon commands.
- Repair prepares the consented block before a single public daemon stop.
- Cancellation/timeout kill only owned helpers; submitted activation may be unknown.
- Strict bounded UTF-8/JSON input/output, redacted public errors, one foreground task.
- Existing Qt protocol/controller/child-process tests continue passing against the
  new engine; new C++ fixtures cover the Rust backend's meaningful failure cases.

Real Desktop launch/shared-service interruption and clean-machine acceptance are
separate from automated fixtures and are recorded honestly, never inferred from builds.

## Implementation record

- `62a15d8`: assessment and shared C++ interfaces.
- `536e1a8`: strict configuration, consent and proxy-file core.
- `9db79ba`: native Windows discovery/activation and bounded helpers.
- `c58211f`: shared launch pipeline, bridge, console/CLI and CMake tests.
- `9ddd630`: remove Rust/Cargo and switch canonical portable packaging/docs.
- Final hardening closes invalid-config authorization loss, edited-block handling,
  extended-path I/O, writer cancellation and configuration-free worker dispatch.
- The file transaction decision closes the external rename-save race; its NTFS
  and Windows API lifecycle constraints are explicit in ARCHITECTURE.md.

See CPP_ENGINE_ACCEPTANCE.md for verification scope, release gates and limitations.
