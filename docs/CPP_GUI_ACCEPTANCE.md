> Historical record for the Rust-backed releases through 0.5.0-rc.1. Current C++ implementation and validation are documented in CPP_ENGINE_MIGRATION_PLAN.md and CPP_ENGINE_ACCEPTANCE.md. Historical test/launch results below do not validate the new engine.

# Qt GUI acceptance — 0.5.0-rc.1

Execution date: 2026-10-03 (Asia/Shanghai). Platform: Windows 11 x64.
Toolchain: MSVC x64, Qt 6.8.3 dynamic, C++20, Rust workspace lockfile.

## Automated evidence

- Rust format, Clippy with warnings denied, and all 161 workspace tests: PASS
  after configuration transaction and launch lease hardening.
- Qt protocol, fake-client controller and real QProcess/fake-engine suites: PASS.
  They cover strict UTF-8 and framing, unknown states, ID correlation, crash and
  restart, output limits, single foreground work, confirmations, cooperative
  shutdown and both cancel/completion response orders. They do not launch Desktop.
  Both cooperative-shutdown timeout and protocol-failure followed by a hung cleanup
  are exercised against real fake-engine children using the production 20s budget.
- `cargo audit`: exits successfully with the existing `lru 0.18.1`
  `RUSTSEC-2026-0253` allowed unsoundness warning. This release does not remove or
  newly suppress the warning.
- CodeGraph: index current. Diff whitespace check and UI mechanical detector: PASS.
- Native Qt Windows render fixtures: PASS at device pixel ratios 1.0, 1.25, 1.5
  and 2.0, light/dark, 520×430 and 480×400 logical sizes. Screenshots under
  `target/gui-review-final/` are local review evidence, not golden test assertions.
- Tab-focused Refresh + Enter invokes Refresh, not Launch; tested in controller
  tests. Confirmation defaults to Cancel and Home is selectable/read-only.

## Independent review

Protocol/lifecycle/configuration review is performed separately from implementation.
The unavailable dedicated Impeccable reviewer role is substituted by that same
independent read-only reviewer for the screenshots and UI code. The mechanical
detector found no issues; native render review found and corrected a clipped Edit
button. Other identified fixes include native Enter behavior, visible primary
button focus, scrollable error details and obsolete cancellation acknowledgments.
Final independent visual review found no remaining material UI issues. The Windows
configuration handle/launch lease changes were independently checked across all
production call sites. Idle-window close reentrancy was caught by native package
smoke, fixed with queued close delivery, and regression tested.
Qt package provenance additionally verifies the official SDK archive SHA and uses
its complete contents in a fresh isolated build SDK, covering headers, import
libraries and CMake configuration as well as deployed runtime binaries. The
archive-to-runtime manifest verifier was executed successfully before packaging.

## Explicitly not claimed

- Windows 10 execution, clean-machine Windows without any SDK/runtime installation,
  assistive screen-reader exercise and multi-monitor movement: NOT RUN.
- Real Desktop ordinary launch, backend Home modification/revoke, shared-daemon
  repair interruption and live network/business flows: NOT RUN for this GUI change.
  These retain the separate package-identity acceptance matrix. Automated fixtures
  verify backend gates/transactions without touching the user's Home.
- Test-driven DPI rendering is not a claim of complete manual UX acceptance.
- Package loader smoke with an SDK-free PATH is not a clean-machine VM test.
- Unsigned executables are not claimed to have Authenticode verification.

## Final release evidence

Canonical `scripts/build-portable.cmd` and `scripts/build-gui.cmd` have completed
successfully from pushed, clean commits. The GUI pipeline rebuilds the canonical
engine, runs all three CTest suites, deploys only Core/Gui/Widgets and their needed
plugins, verifies the unmodified Qt DLL hashes, and includes corresponding source.

Package smoke creates the GUI and closes it without starting an engine; separate
engine hello/shutdown smoke holds stdin open, uses an isolated temporary Guard
configuration, and verifies the file remains unchanged. Both EXEs report matching
product version/commit/dirty state. Smoke uses a PATH without Qt/VS/Rust SDK paths.
An import-table inspection found the engine's VCRUNTIME140 dependency; the package
therefore also places that runtime beside the engine and runs its smoke from its
own directory, rather than depending on the GUI's working directory.

Output: `dist/CodexProxyGuard-0.5.0-rc.1-windows-x86_64.zip`, matching `.sha256`,
`dist/gui/SHA256SUMS.txt`, `dist/gui/build-info.json`, and the canonical Rust files
under `dist/portable/`. The generated build-info is authoritative for each rebuild's
exact commit, per-file hashes and SDK/source provenance. Artifacts remain ignored.

Known build observations: the QtBase-only SDK has no translations catalog (the
English-only GUI deliberately uses `--no-translations`); the canonical Rust script
reports Authenticode status unavailable in this PowerShell environment. Neither is
recorded as successful signing or full clean-machine acceptance.

---

# UI/UX remediation — 2026-10-03 (current C++ implementation)

Scope: `gui/src/ui/**` only, following the 2026-10-03 UI/UX audit (since removed after
remediation). No change to `backend/`, `app/`, the protocol, the state machine,
`DESIGN.md` or `PRODUCT.md`. No animation was added; the indeterminate progress bar
remains the only motion and it was already backed by three static cues.

## Contract evidence (in-repo assertions)

The following are encoded as GUI test cases so they hold without re-running a
manual review:

| Finding | Assertion | Test |
|---|---|---|
| #1 Launch unexplained when disabled | every clause of the engine's `safeToLaunch` (config readiness, desktop missing, already running, elevation, unknown method) states a reason; several at once are all listed; a launchable state states none | `blockedLaunchStatesItsReason` |
| #2 Primary focus ring invisible | accent-on-surface, accent-on-hover and window-on-accent all clear 3:1 | `themeFillsClearContrastFloors` |
| #3 Control borders invisible | `border` clears 3:1 on `window`, `surface` and `hover` in both schemes | `themeFillsClearContrastFloors` |
| #4 Machine codes shown to users | the human message is reachable; the code moves to the tooltip | `statusLineStaysReadableAndBounded`, `proxySaveFailureKeepsDialogAndInput` |
| #6 Long messages broke 480×400 | a ~180-character message leaves the window at its minimum height | `statusLineStaysReadableAndBounded` |
| #7 Shortcut table drifts from bindings | every live `QShortcut` key appears in the About table | `aboutShortcutsMatchRegisteredBindings` |
| #8 Wrong first tab stop | Launch owns the initial focus after show; the row-level `Edit` is `NoFocus` | `primaryActionOwnsFirstTabStop` |
| #12 Unknown states hidden | an unrecognized coverage state and launch method both stay visible | `unknownEngineTokensAreNotFlattened` |

## Verification actually run (2026-10-03)

- `scripts/test-cpp.ps1` (dynamic SDK, MSVC 18 / Qt 6.8.3): **10/10 CTest suites
  passed**, including the 31 assertions of `controller_tests`.
- `renderReviewFixtures` with `CPG_SCREENSHOT_DIR`: light/dark captures at 480 and
  520 logical width on a device pixel ratio of 2.0, followed by pixel-band
  measurement of the captures: the information-card border measures 3.74:1
  (dark) / 3.86:1 (light) against the window, secondary-button outlines
  ~3.4:1+ against their fill, and the focused Launch ring (window-on-accent)
  5.98:1 (light) / 7.58:1 (dark) — the HIGH findings #2/#3 are verified as
  rendered, not only computed.
- `git diff --check`: clean.

Measured contrast values are recorded as comments at their definition site in
`gui/src/ui/theme.cpp` (sRGB relative-luminance formula, 3:1 non-text / 4.5:1 text).
The remediation raised `border` to dark `#6a88a3` / light `#6b7f8f`; that change's
knock-on collision with the progress chunk (1.72 / 1.55) was found by recomputation
and fixed in the same edit, not by eye.

## Explicitly not claimed

- Assistive screen-reader exercise (NVDA/Narrator), Windows 10, clean-machine and
  multi-monitor: NOT RUN.
- Real Desktop launch, backend Home modification and live network flows: NOT RUN.
- The audit document itself was deleted after remediation per its own batch plan;
  the table above is the durable record of its findings.
