# TUI acceptance — 0.7.0-rc.2 (no-Qt)

Execution date: 2026-10-11 (rc.1 evidence) and 2026-10-12 (rc.2 hardening
evidence) (Asia/Shanghai). Platform: Windows 11 x64, MSVC x64, static CRT
release profile.

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
