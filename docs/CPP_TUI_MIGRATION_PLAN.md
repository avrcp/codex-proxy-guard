# Win32 TUI / no-Qt migration plan — 0.7.0-rc.1

Execution record for the `feature/win32-tui-no-qt` series. Goal: one Console
subsystem EXE (C++20, static CRT `/MT`, no Qt, no PowerShell APPX discovery)
with an in-process Win32 VT TUI, keeping every verified engine security
semantic unchanged. Rollback: tag `qt-gui-v0.6.1-rc.1` (see
`docs/history/QT_GUI_HISTORY.md`).

## Non-negotiable invariants (carried from the Qt era)

- Config schema version 2 round-trip; unknown fields/types, `version != 2` and
  non-UTF-8 are refused; authorization bindings are never silently dropped.
- `.env` semantics: 1 MiB limit, strict UTF-8, single-line dotenv lexical gate
  (`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED`), marker-pair strictness, outside
  bytes preserved, expected-content recheck inside an isolated TxF,
  revoke-before-consent-save, fail closed when TxF is unavailable.
- Launch pipeline ordering, startup lock, elevation gates, single foreground
  operation, activation-worker outcome semantics (post-submission aborts are
  `APPX_ACTIVATION_OUTCOME_UNKNOWN`, never auto-retried), AppModel activation
  identity verification, zero daemon side effects on normal launch.
- Strict JSON: size limit, strict UTF-8, root object, duplicate-key rejection
  at every nesting level, no comments, no trailing garbage.
- JSON/text hygiene on display: `publicText()` redaction parity (USERPROFILE,
  ANSI/OSC, URI credentials, secret-bearing lines, control characters).

## Environment-driven adaptations (deviations from the ideal 9-commit ladder)

1. **Qt availability.** The machine executing this migration had no Qt SDK
   initially; a dynamic Qt 6.8.3 `msvc2022_64` SDK was cached at
   `D:\Qt\6.8.3\msvc2022_64` (aqtinstall, qtbase) so every commit can be gated
   by both builds. The interim Qt regression build is auto-detected
   (`find_package(Qt6 ... QUIET)`), never required; it is removed with the GUI.
   The pinned static Qt SDK pipeline is not rebuilt (hours, and the final
   product no longer needs it).
2. **Commit granularity.** The engine (`core`/`platform`/`launch`/`bridge`/
   `cli`) migrates in one commit (manual steps 3+4 combined) because the
   engine's public types change atomically; its QtTest suites port to doctest
   in the same commit. PowerShell discovery keeps running across that commit
   (script re-embedded without Qt resources) and is replaced by native
   AppModel discovery in the next commit, so discovery parity is attributable
   to exactly one change.
3. **Classic family name.** `OpenAI.ChatGPT-Desktop` is not installed on this
   machine, so its family name cannot be measured here. It is derived by
   construction (family = `<Name>_<PublisherId>`, PublisherId identical for the
   same publisher, measured `2p2nqsd0c76g0` from the installed
   `OpenAI.Codex_2p2nqsd0c76g0`). A wrong family yields zero records and
   `CODEX_NOT_INSTALLED` (fail closed); it can never select a wrong target
   because package identity is re-derived from `PackageIdFromFullName`.
4. **Signing.** No certificate is available to the executor; the packaging
   script implements the signtool sign/verify gate and records
   `authenticode_status` in `release-manifest.json`. Unsorted development
   artifacts are explicitly marked unsigned.

## Commit ladder (as executed)

| # | Commit | Gate beyond build+tests |
|---|--------|--------------------------|
| 1 | `chore(migration)` baseline freeze | tag `qt-gui-v0.6.1-rc.1`; capture `backend/tests/fixtures/appx_discovery_baseline.json` from the real PowerShell discovery on the reference machine (OpenAI.Codex `26.1007.2314.0_x64__2p2nqsd0c76g0`, applications `App`→`app/ChatGPT.exe` and `CodexCoreCommandRunner`); legacy Qt suite green on the dynamic SDK |
| 2 | `feat(foundation)` STL/Win32 primitives | nlohmann/json 3.12.0 vendored (URL+SHA-256 recorded); doctest test-only; foundation unit suites |
| 3 | `refactor(engine)` off Qt Core | doctest ports of core/platform/launch/bridge suites; Qt regression build still green (GUI drives the unchanged NDJSON bridge); no-Qt build green |
| 4 | `feat(discovery)` native AppModel | parity fixture vs. commit 1 baseline; no PowerShell fallback; `powershell.exe` never spawned. Executed 2026-10-11: live A/B on the reference machine — native discovery vs. a fresh run of `resources/appx-discovery.ps1` returned identical envelopes for OpenAI.Codex 26.1007.2314.0 (every field, including both manifest applications; the Appx reader's backslash canonicalization of `Executable` is normalized to the baseline's forward-slash spelling) |
| 5 | `feat(tui)` Win32 VT TUI | in-process session; plain fallback; key/resize/modal/width suites; single foreground operation; Ctrl+C cancel |
| 6 | `chore(qt)` removal | `rg "#include <Q\|Qt6::\|find_package\(Qt\|Q_OBJECT\|QApplication\|QCoreApplication"` empty over production+test sources |
| 7 | `chore(release)` packaging/gate/docs | import-table scan (system DLLs only), size report, signtool gate, docs synchronized, `0.7.0-rc.1` |

## Third-party policy (production)

- toml++ 3.4.0 (MIT) — kept, pinned.
- nlohmann/json 3.12.0 (MIT) — vendored single header, recorded SHA-256,
  no FetchContent at configure/build time.
- doctest (MIT) — test binaries only, never linked into production.
- Windows SDK / STL only otherwise. No Rust, FFI, packers, UPX, obfuscators,
  self-extracting shells or hand-written PE loaders.

## Text and path conventions

Protocol/config/error/public-JSON strings are UTF-8 `std::string`; filesystem
paths are `std::filesystem::path`; Win32 boundaries use `std::wstring`;
raw `.env` bytes are binary-safe `std::string`. No `std::u8string`/`char8_t` in
business types; no `std::regex` over untrusted input (deterministic scanners).

## Completion standard

The 18-point list from the working brief (no Qt source/binary, no PowerShell
discovery, no bridge self-child, default no-arg TUI, VT-unavailable plain
fallback, config v2 compatibility, `.env`/activation/outcome semantics
preserved, single static-CRT EXE ≤ 8 MiB (target ≤ 5), no packer, Authenticode
gate for public releases, suites migrated, double-click smoke, receipt parity,
AV evidence) must hold before `0.7.0-rc.1` is released; deviations are recorded
above and in the release manifest.
