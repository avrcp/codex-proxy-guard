# TUI acceptance — 0.7.0-rc.1 (no-Qt)

Execution date: 2026-10-11 (Asia/Shanghai). Platform: Windows 11 x64,
MSVC x64, static CRT release profile.

## Automated evidence (in-repo suites, all green)

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
- The 0.7.0-rc.1 artifact is an UNSIGNED DEVELOPMENT BUILD until a
  certificate is provided through
  `scripts\build-portable.cmd -Sign -CertificateSha1 <thumbprint>`.
