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
