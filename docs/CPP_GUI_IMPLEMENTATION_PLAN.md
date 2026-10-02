> Historical record for the Rust-backed releases through 0.5.0-rc.1. Current C++ implementation and validation are documented in CPP_ENGINE_MIGRATION_PLAN.md and CPP_ENGINE_ACCEPTANCE.md. Historical test/launch results below do not validate the new engine.

# C++ / Qt GUI implementation plan

Baseline: `04142556bd57d2d69fa25e0344d4ea24a85e0fa0` (`0.4.1-rc.6`).
Target: `0.5.0-rc.1`, Windows x64, C++20 / dynamically linked Qt 6 Widgets.

The supplied implementation brief defines the requested frontend. Existing Rust
security invariants remain authoritative. No Rust business logic is rewritten in
C++; the TUI and CLI remain available. No Telegram source or assets are imported.

## Implementation sequence

1. Add and test a bounded schema-1 stdio bridge over the existing reducer,
   authorization and dispatcher. Bind consent and repair to single-use server
   confirmation tokens, including the exact displayed Home and configuration.
2. Add the Qt process client, strict protocol decoding, handshake and read-only
   snapshot. Resolve the engine only relative to the GUI installation.
3. Add controller-driven launch, refresh, proxy editing and scoped consent dialogs.
4. Add repair confirmation, cancellation, graceful close and explicit engine restart.
5. Test protocol and controller failure paths using a fake client; run Rust gates.
6. Synchronize architecture, security, release and usage documentation; review diffs,
   commit and push the development branch, then build canonical portable artifacts.

Independent environment/package preparation runs alongside implementation. Qt
installation is authorized by the user; no machine-wide proxy or Codex settings
are changed. Real shared-daemon repair is not an automated test.

## Evidence and limits

Automated tests cover framing, limits, unknown states, request correlation,
single-operation behavior, confirmation, cancellation and controller affordances.
Packaging must record source commit/dirty state, Qt/compiler versions and hashes.
Use `scripts/build-portable.cmd` for Rust and `scripts/build-gui.cmd` for the GUI.
Native window, DPI and clean-machine acceptance are tracked separately from unit
tests. Do not infer network coverage or successful Desktop activation from a build.

Final execution evidence is recorded in `CPP_GUI_ACCEPTANCE.md`.
