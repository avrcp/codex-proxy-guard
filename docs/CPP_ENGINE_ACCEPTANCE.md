# C++ engine acceptance — 0.6.0-rc.1

Date: 2026-10-03. Baseline: 0.5.0-rc.1 (`63e0ffc`).
Branch: `codex/cpp-engine-migration`.

## Scope delivered

All Rust crates, Cargo manifests/lockfile and Rust build paths were removed.
C++20 now implements config/domain validation, `.env` transactions, Windows
APPX/process/elevation/activation, daemon compatibility, launch orchestration,
stdio bridge, CLI and console. GUI and engine build together under CMake/MSVC.
The old full-screen TUI was replaced by a line-oriented console; the Qt GUI is
the primary interface. Config schema and existing authorization bindings remain
readable for data safety, with no Rust binary fallback.

## Verification

The Windows developer build uses MSVC C++20 `/W4 /WX`, Qt 6.8.3 and CTest.
All seven suites passed: backend core, platform, launch, bridge, GUI protocol,
controller and child-process lifecycle. The canonical release script builds and
reruns those same suites using a separately extracted, checksum-verified official
Qt SDK. Detailed local CTest logs are under `target`; package provenance/checksum
records are generated in `dist/gui/build-info.json` and `SHA256SUMS.txt`.

Additional observed checks:

- Real, read-only production discovery identified installed ChatGPT Desktop,
  its FullTrust `App` / `app/ChatGPT.exe` entry and the existing running process.
  No launch, process termination or daemon stop was issued.
- Real bridge hello/shutdown exits with stdin kept open and does not create its
  isolated config. An unread stdout pipe causes classified exit in about two
  seconds rather than pinning the bridge; this also has a native pipe regression.
- toml++ 3.4.0 source SHA-256 matches the pinned upstream header. Release packaging
  checks it again and includes its MIT license.
- CodeGraph status was available; source, compiler and tests are authoritative
  for the migrated C++ implementation.

## Independent review and repairs

Review covered state/cancellation, single-use confirmation scope, held config
leases, helper budgets/ownership, activation uncertainty, repair ordering and
persistence failure paths. Resulting fixes include:

- Reject invalid-config recovery/reset when authorization fields may survive in
  the damaged bytes, including escaped keys and invalid UTF-8.
- Reject a marked block whose endpoint/no-proxy values could not be generated
  by Guard; failed prepare/revoke preserves file bytes and authorization.
- Normalize native file separators for legacy extended Windows Home paths and
  exercise actual create/update/revoke operations with an extended drive path.
- Remove hidden activation worker and build-info dependency on default config
  directory resolution.
- Replace verify-then-path-rename with an NTFS file transaction. Deterministic
  fixtures run a separate editor process after verification and attempt rename,
  rename-save and writer access; rollback fixtures preserve file/consent state.

## Release gates and limitations

Build only through `scripts/build-portable.cmd` from the pushed clean commit.
The script checks packaged GUI/engine version, commit and dirty state, executes
GUI and bridge smoke under an isolated system PATH, verifies deployed Qt hashes,
and emits a ZIP SHA-256 plus all package-member hashes. The delivered artifact's
`build-info.json` is the exact commit-to-binary record; do not substitute a
historical Rust release's test or launch results.

Backend `.env` changes require local NTFS transactions. TxF is a deliberately
narrow dependency for strict concurrent-file preservation; Microsoft recommends
alternatives and may remove it in future Windows versions. Unsupported locations
fail closed without a nontransactional fallback. See the architecture decision.

Not exercised: live Desktop activation, live shared-daemon interruption, a clean
Windows VM, Windows 10, UNC storage, high-DPI/screen-reader acceptance or Authenticode
signing. Read-only discovery, fixtures and portable smoke do not establish network
connectivity or prove the backend consumed the proxy block.

## Remediation round — 2026-10-03 review

Baseline `c2102d1`; four fixes landed on top (`6013a0f`…`95ce6c0`), each with
regression tests added first. The full suite was rerun after every stage.

- R1 `fix(gui): close cleanly after engine has already stopped`. The controller
  previously waited only for a future `stopped` notification, so after a start
  failure or post-crash cleanup the window could never close. It now records the
  terminal stop fact (cleared before each `start()`), and `closed` completes from
  that fact exactly once per close. Reproduced on the pre-fix controller: the
  real-bridge start-failure, crash, and window-close cases failed before the fix.
  `EngineBridge::notifyStopped()` one-shot semantics are unchanged
  (`shutdownBeforeStart` still passes).
- R2 `fix(core): reject unsafe dotenv lexical boundaries`. `readEnv()` scanned
  physical lines without quote context, so a marker block inside a multi-line
  quoted value was accepted as a real block — inspection reported `current` and
  revoke would have removed the user's string bytes (reproduced on the pre-gate
  core: the quoted-template fixture reported `current`). A bounded read-only
  lexical gate now refuses multi-line quoted values, continuations and
  unterminated quotes with `BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED` before marker
  recognition: no byte changes, inspection reports the new `unsupported` state
  ("Cannot auto-edit · fix .env manually", never a network claim), and a failed
  revoke keeps consent with the specific cause. TxF transaction, concurrency and
  rollback fixtures are unchanged and still pass.
- R3 `fix(gui): surface reused and unknown activation instances`. The receipt's
  `instance` classification now drives distinct success messages (reused warns
  that this launch's proxy settings may not have been re-applied) and a Details
  row; the automatic post-completion snapshot refresh no longer swallows the
  warning. Backend receipts still classify created/reused/unknown with a single
  activation per launch and no retry.
- R4 `fix(gui): retain proxy edits until save is confirmed`. The proxy editor
  is now a `ProxySettingsDialog` shown with `open()`; Save submits and waits for
  the engine-bound `set_proxy` result (`proxySaveSucceeded` / `proxySaveFailed`
  / `proxySaveOutcomeUnknown`). Validation, lock, write and changed-config
  failures keep the draft in place for in-place correction; Cancel/Esc/X are
  blocked only while a save is pending; engine failure mid-save reports an
  unconfirmed result without resubmitting. A discovery error in the refreshed
  snapshot after a confirmed save is reported separately from the save result.

Verification this round: `scripts/test-cpp.ps1 -QtRoot C:\Qt\6.8.3\msvc2022_64`
passed all seven suites (backend core, platform, launch, bridge, GUI protocol,
controller, engine bridge) with zero failures after each remediation stage;
`git diff --check` clean. Pre-fix behavior was demonstrated for R1 and R2 by
re-running the new tests against the unfixed sources. Still not exercised,
unchanged from above: live Desktop activation, live shared-daemon interruption,
clean Windows VM, Windows 10, UNC, high-DPI/screen-reader and signing acceptance
(V1 manual matrix remains pending operator authorization).

Final release artifact: after the packaging-script hardening (`ef159bf`), the
whole `dist/` tree was rebuilt from scratch. `scripts/build-portable.cmd` ran
from clean commit `ef159bf` on `main` (`git_dirty=false`; the script re-ran all
seven suites, 100% passed, and — new in this round — executed
`scripts/verify-package.py` itself as the final gate: 129 manifest members,
ZIP CRC passed). `dist/gui/build-info.json` records product `0.6.0-rc.1`, Qt
6.8.3, C++20 engine, GUI and engine commit `ef159bf`, deployed Qt DLL hashes
equal to the verified official SDK, and engine bridge smoke (hello/shutdown
passed; stdin held open; isolated config unchanged). ZIP:
`CodexProxyGuard-0.6.0-rc.1-windows-x86_64.zip`, SHA-256
`16e0de0d542e31b9b8a01a6114158c9c76d55d98772090d85f5c4ed87ba67639`
(independently recomputed; member manifest in `dist/gui/SHA256SUMS.txt`). The
earlier `ba176b1` ZIP was discarded together with the whole old `dist/` tree;
the canonical script now also removes its per-run Qt SDK extraction and smoke
scratch directories, and `target/` holds no Rust-era artifacts (`CACHEDIR.TAG`,
cargo `debug`/`release`/target-triple directories, rust test logs and probe
fixtures were deleted; `aqtinstall.log` removed from the repository root).
