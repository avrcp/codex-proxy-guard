# TUI acceptance — 0.7.0-rc.4 (no Qt)

Execution dates: 2026-10-13 (rc.4 verifier-fix pass and rc.3), 2026-10-12
(historical rc.2) and 2026-10-11 (historical rc.1), Asia/Shanghai. Platform:
Windows 11 x64, MSVC x64, static CRT release profile.

## RC4 release-validation corrections

The RC3 review found the release verifier itself defective, and rc.4 fixes
it before any further acceptance is claimed:

- The independent Authenticode gate could never execute: `byref()` was
  assigned into the typed `WINTRUST_DATA.pFile` field (TypeError before
  `WinVerifyTrust` was ever called) and the cleanup pass used state action
  16 while `WTD_STATEACTION_CLOSE` is 2. rc.4 carries a SDK-shaped x64
  structure set, a typed file pointer, a VERIFY→CLOSE lifecycle in
  `finally` where only status 0 is valid and cleanup failures never mask
  the primary result — with a fake-API lifecycle matrix plus real
  WinVerifyTrust controls (unsigned fixture rejected, embedded MS-signed
  vswhere.exe accepted). Publisher identity now comes from the signature
  object, not from parsing localized signtool text.
- The mitigation gate accepted the APPCONTAINER DllCharacteristics bit and
  an unrelated GuardFlags bit as CET evidence. The rc.2 statement below
  about "the extended-characteristics debug record spelling newer linkers
  use" described one of *three accepted spellings* — that claim is
  **withdrawn**: the type-20 extended record is the only evidence rc.4
  accepts, per the C01–C09 negative matrix and a real
  `/CETCOMPAT` vs `/CETCOMPAT:NO` toolchain fixture pair. PE parsing is
  strict x64/PE32+ with bounds checks; truncated records fail closed.
- `cet_compat=true` states the image is marked CET-compatible; it does not
  claim hardware shadow stacks were active at run time. The PE CFG flags
  prove the image's table, not that every object was compiled with
  `/guard:cf` — object-level coverage comes from the shared build options.
- A build-host change (VS 18 cl no longer honors `VSLANG`; CMake 4.3
  silently drops `rules.ninja` for a plain-variable non-ASCII
  `/showIncludes` prefix) had broken every Ninja configure; the prefix is
  now stored via the cache-variable path and header-dependency tracking
  was re-verified.

## RC4 automated acceptance

Execution: 2026-10-13, Windows x64, MSVC 19.51 (toolchain 14.51), CMake
4.3.1, static CRT. Full CTest: 11/11 suites passed, 0 skipped — the ten
rc.3 suites plus the new CET toolchain fixture pair, with the
verifier-fixture suite extended to 54 cases (WinTrust lifecycle matrix,
real WinVerifyTrust controls, manifest signature-fact rejections) and the
release-input suite to 14 cases (signature-metadata extraction with a
shadowed cmdlet). Console-event tests only pass in a process tree without
the inherited Ctrl+C-disabled state: agent harness sessions carry that
state into every descendant, which silently suppresses CTRL_C_EVENT
handler delivery (reproduced minimally and cleared with
`SetConsoleCtrlHandler(NULL, FALSE)`). The rc.3-era binary behaves
identically under such a tree, so a red run there is an execution-context
artifact, not a regression; run the gate from a normal interactive
console or a scheduled task outside the harness tree.

Current readiness is recorded in [release readiness](RELEASE_READINESS.md).

## RC3 correction to the inherited evidence

The rc.2 statement that this host strips the CreateProcess handle list was
incorrect. The code constructed the list but never assigned
`STARTUPINFOEXW::lpAttributeList`. Its inheritable sentinel test then returned
without failing on a leak. The historical claim below is retained for audit,
but it is **withdrawn as evidence of restricted inheritance**. RC3 assigns the
attribute list, removes the waiver, and tests that the sentinel cannot be used.
Both that test and deterministic post-child-exit reader verdict tests failed
on the old implementation before repair. No host skip is counted as PASS.

The final main gate also exposed a probe false positive: the child reused the
parent's numerical handle value for a different object (exit 2), while the
parent sentinel remained unsignalled. The probe now compares kernel object
identity, with a deliberate broad-inheritance positive control and an explicit
different-object case. A real sentinel leak still fails unconditionally.

The final gate also reproduced `TerminateProcess` returning access denied
during child exit even though all held handles settled 6 ms later. Cleanup
now uses final completion facts; transient termination/cancellation errors do
not replace the original output-limit result. Actual wait failures and cleanup
budget overruns remain failures, with stage, native error and elapsed-time
diagnostics. Deterministic regressions cover both sides of this boundary.

The rc.2 CLI empty-config statement covered the CLI path, not the default
interactive classifier. The old component tests did not execute the failing
first-B production handler. RC3 replaces the classifier and JSON UI boundary
and tests the production controller with a real session and injected services.

RC3 plans and source scope are in [RC3 remediation](RC3_REMEDIATION.md).

## RC3 automated acceptance

Execution: 2026-10-13, Windows x64, MSVC 19.51.36257, CMake 4.3.1,
static CRT, `/W4 /WX /sdl /guard:cf`. The fresh integrated development build
passed all 10 registered suites with zero skipped doctest cases. Follow-up
review fixes are included in the final clean-commit verification and the
canonical package reruns these suites from its fixed source archive.

| Requirement | Implementation | Regression evidence | Result |
|---|---|---|---|
| R01 E01–E04 | `app/main.cpp`, `backend/src/command_line.*`, `cli.*`, `cpg/port.h` | `app_command_line`: default resolver injection, Unicode/relative paths, production dispatch, invalid options/strict ports; `backend_engine_cli`: headless output and isolated EOF | PASS |
| R02 A01–A04 | `session.*`, `tui_controller.*`, `tui_state.*` | Real-session first B/open/cancel/confirm; absent/null entry render; frozen/stale proposal; failed config read burns token; controller clears stale view | PASS |
| R03 P01–P06 | `foundation/src/process.cpp` | Strict identity sentinel; concurrent endpoints; delayed reader; partial/zero write; setup failures; drain descendant survives; final-write cancel race; transient cleanup API errors vs actual wait/budget failure | PASS |
| R04 U01–U04 | `session.*`, `tui_controller.*`, `tui_state.*` | Host/Port Enter; failed draft retention; committed refresh failure; immediate busy; field-specific errors; operation ID | PASS |
| R05 L01–L03 L05–L06 C01–C02 | `launch.*`, `session.*`, controller/result rendering | Normal launch has zero resolve/stop; prepare failure stops nothing; stopped daemon facts survive failure; reused/unknown; one refresh; late verified receipt retained | PASS |
| R06 C03–C04 T01–T02 | `console_control.*`, terminal/input/plain/controller | Private-console Ctrl+C/Break; busy Q defaults to keep; EOF/invalid output/tiny viewport; real WM_CLOSE owner settle and terminal restore before acknowledgement; idle redirected pipe cancellation without EOF; unsupported character devices fail closed | PASS |
| R07 U05–U07 | `tui_state.*`, `tui_controller.*` | Long Unicode scope scroll, border/input clipping, caret; paired code/message diagnostics; clipboard failure preserves original PID/daemon facts | PASS |
| R08 D01–D02 L04 | `package_discovery.*`, `platform.cpp`, `launch.cpp` | Secondary optional null; required FullTrust; retries/count/bytes/deadline/cancel; image/elevation failure; conservative instance timestamp | PASS |
| R09 B01–B05 | `build-portable.ps1`, `test-noqt.ps1`, `test_release_inputs.py` | Outside-root/version/lock/junction guards; fixed git archive, unique intermediates; temporary plain config; unsigned path; 23 verifier fixtures including UTF-8 execution on a GBK host; final package verifier | Source guards PASS; artifact verification recorded in release manifest/final delivery |

The earlier “host strips HANDLE_LIST” explanation is not used by any current
assertion. New synchronization and fault seams are compiled only into the
foundation test target; production exposes no seam command or environment flag.
The console control tests use dedicated hidden test consoles and fake work,
not a live Desktop or shared daemon. Config/consent tests use disposable paths.
Native and redirected-pipe cancellation tests synchronize with completed reader
transitions (ready, character accepted, cancellation applied), rather than
assuming progress after a fixed sleep. The observer is explicit internal
dependency injection and is absent from normal production calls.

## RC3 real application and distribution scope

| Scenario | Status |
|---|---|
| Default resolution and actual production controller B/save/launch/cancel using isolated services | PASS |
| Headless production EXE Unicode path, JSON errors, plain EOF | PASS |
| Explorer double click of the final EXE | NOT RUN |
| Human attended Windows Terminal / conhost resize and clipboard paste | NOT RUN |
| Live Desktop normal launch and actual created/reused receipt | NOT RUN |
| Real Codex Home authorize/revoke/port changes | NOT RUN |
| Live shared-daemon repair and interruption of other clients | NOT RUN |
| Windows 10, DPI/multi-monitor, Defender/SmartScreen/third-party AV distribution | NOT RUN |
| Authenticode signing / public release | NOT RUN; unsigned development package only |

No real-application or distribution guarantee is inferred from the isolated
regressions. Guard never terminates the activated Desktop, and cancellation
with unknown submission outcome never triggers an automatic retry.


## 0.7.0-rc.2 hardening evidence (executed 2026-10-12)

The rc.2 hardening pass extended the automated suites; everything below ran
green in the same session as the change:

- Cluster-safe cell model: emoji written atomically at the right frame edge,
  an unfittable emoji is not partially written, combining marks attach to
  their leading glyph (including after wide glyphs), zero-width clusters at
  the clipped boundary never address out-of-bounds cells, isolated
  surrogates normalize to U+FFFD, cluster-aware truncation never splits a
  glyph, and unicode homes render/clip without escaping the frame.
- Session timeout reaping: with a 20 ms injected `SessionBudgets` budget the
  session stays busy and refuses a second foreground operation while the
  timed-out future is reaped, surfaces `SESSION_TIMEOUT` only afterwards,
  and never overlaps effects (`maxActive == 1`).
- Owned-process runner: the child inherits only its standard handles
  (non-inheritable control probe always passes; the strict inheritable-probe
  assertion runs wherever the host honors the CreateProcess attribute list —
  this dev host strips it, where the only-inheritable-duplicates
  construction is the barrier), a child that never reads 16 KiB of stdin
  still hits `HELPER_TIMEOUT` promptly, a fast-exiting flood is caught by
  the output limit in 50/50 iterations, and custom environment blocks are
  ordinal-sorted with duplicates collapsed.
- Clipboard ownership: checked EmptyClipboard/SetClipboardData semantics
  with the HGLOBAL freed exactly when ownership does not transfer (fake
  adapter), plus the transient "Diagnostic copied" notice.
- CLI contract: `L` is bound as the documented launch shortcut, modal
  openers respect the snapshot capability flags (fail closed pre-snapshot),
  `--plain` off the interactive mode and an empty `--config` value are
  `CLI_INVALID`, and usage no longer advertises the removed bridge role.
- Verifier: 20 python fixtures including source-ZIP digest/manifest/sums
  agreement, build products and `target/` members rejected, exact artifact
  set enforced, and the PE mitigation gate (CFG instrumented with FID table,
  ASLR, NX, high-entropy VA, CET — including the extended-characteristics
  debug record spelling newer linkers use).
- Release chain: promotion is immutable (an existing destination refuses the
  build), staging is unique per run, promotion is one atomic move, and the
  manifest carries verifier-derived mitigation facts.

## Automated evidence (rc.1 baseline, in-repo suites, all green)

- All 8 CTest suites on the no-Qt build: foundation, startup classification,
  TUI (display width incl. CJK/surrogates/combining marks, screen clipping,
  input normalization, key mapping, proxy editor semantics, YES-typed
  single-use confirmations, busy-state gating, blocked-reason text, error
  modal follow-ups, frame drawing bounds, shared engine session), engine
  core, engine platform (incl. the native discovery parity gate against the
  frozen PowerShell baseline), engine launch, engine CLI and the 12 python
  verifier fixtures.
- The interim Qt regression configuration (legacy GUI driving the migrated
  engine through the unchanged bridge protocol) was green at 12/12 before
  the Qt assets were removed; recorded in the migration plan commit ladder.
- The unified EXE measured 1.09 MiB (Qt static baseline: 18.3 MB), console
  subsystem, import table limited to Windows system DLLs; `--version`,
  `build-info` and `licenses` smoke verified from the built artifact.
- `console --plain` line-mode smoke verified over redirected stdio; the
  non-interactive fallback (no VT console) automatically selected the plain
  console in the same run.

## Real-console probe evidence (executed 2026-10-12)

A dedicated probe (target/probe, not committed) allocated a hidden 100x30
conhost console, spawned the release EXE with real CONIN$/CONOUT$ device
handles, read the rendered screen buffer and injected key events:

| Check | Result |
|---|---|
| Title row "CODEX PROXY GUARD" + key legend rendered via real VT output | PASS |
| `E` opens the Proxy Configuration modal (box + fields rendered) | PASS |
| Esc closes it; `?` opens Help | PASS |
| `Q` quits cleanly (exit 0, console state restored) | PASS |
| Startup stability: process alive and rendering 4s after launch (also verified in a visible console) | PASS |
| Empty-snapshot first frame renders without exception (regression suite) | PASS |

During hardening the probe caught a real defect - the first frame rendered
before the initial session snapshot arrived and nlohmann `value()` threw
type_error.306 on null receivers; fixed by resolving missing snapshot keys
to empty objects and covered by a dedicated regression test.

## Interactive matrix (partially pending)

Executed by the executor: real conhost rendering/modal/quit via the probe
above, PowerShell console launch (the probe runs under PowerShell), plain
fallback under redirected stdio. Still requiring a human-attended session
(tracked by `docs/RELEASE_CHECKLIST.md`):

- Windows Terminal specifically (the probe exercises conhost).
- Explorer double-click console window.
- Ctrl+C during a live foreground operation; window close mid-operation.
- DPI-specific rendering (100/150/200%).

## Explicitly not claimed

- Windows 10 execution and multi-monitor movement: NOT RUN.
- Real Desktop launch receipt, backend Home modification/revoke and
  shared-daemon repair against live Desktop: not exercised by automated
  suites by design; the receipt schema is validated by the engine fixture
  suites and the native discovery parity gate. A live A/B against the
  0.6.1-rc.1 Qt release remains open until the release checklist matrix is
  run on an interactive session.
- Clean-machine Defender/SmartScreen/third-party AV matrix: NOT RUN (no
  signing certificate available to the executor; unsigned artifacts are
  marked accordingly). Vendor false-positive submission is a manual step
  documented in `docs/RELEASE_CHECKLIST.md`.
- The 0.7.0-rc.2 artifact is an UNSIGNED DEVELOPMENT BUILD until a
  certificate is provided through
  `scripts\build-portable.cmd -Sign -CertificateSha1 <thumbprint>`.
- The rc.2 clipboard fix is unit-tested through the injectable adapter; the
  manual Windows Terminal/conhost paste-into-Notepad check is still part of
  the interactive matrix below and is NOT RUN yet.
