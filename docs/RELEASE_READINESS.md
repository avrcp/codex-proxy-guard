# Release readiness — 0.7.0-rc.4

Single place that answers "what state is this candidate actually in". The
four levels are cumulative and must not be collapsed into one another;
automated isolated tests passing is not functional acceptance, and an
unsigned development package passing is not a public release. Statuses are
`YES`, `NO`, `PARTIAL` or `BLOCKED` with the blocking condition named.

| Level | Meaning | Status |
|---|---|---|
| `code_ready` | RC3 review fixes (R01/R02) merged, all automated suites green | YES (see below) |
| `dev_artifact_ready` | Unsigned development package from a clean, pushed commit passes structure/provenance/packaging verification | YES — produced by the canonical script from the merged main commit; facts recorded in `dist/releases/<version>-<commit>/release-manifest.json` |
| `functional_acceptance_ready` | The final EXE has complete real-use acceptance evidence (W01–W14 of the A01 matrix) | NO — automated isolated suites only; the human-attended matrix remains NOT RUN |
| `public_release_ready` | Everything above plus a valid Authenticode signature and distribution/AV acceptance | BLOCKED — no signing credential; A02 has no before/after AV evidence yet |

## RC3 review fixes shipped in rc.4

- **R01 (WinVerifyTrust/signing facts)**: the independent Authenticode gate
  previously could not run at all — `byref()` was assigned into the typed
  `pFile` field (TypeError before any call) and the cleanup used state
  action 16 instead of `WTD_STATEACTION_CLOSE` (2). The verifier now
  carries a SDK-shaped x64 `WINTRUST_DATA`, drives VERIFY→CLOSE in a
  `finally`, treats only status 0 as valid and keeps cleanup failures from
  masking the primary result. Publisher identity, signer thumbprint and
  the timestamp fact come from the signature object
  (`Get-AuthenticodeSignature`), never from localized signtool text.
- **R02 (CET evidence)**: the mitigation gate accepted the APPCONTAINER
  DllCharacteristics bit and an unrelated GuardFlags bit as CET. CET now
  comes only from the `IMAGE_DEBUG_TYPE_EX_DLLCHARACTERISTICS` (type 20)
  debug record; missing/truncated records fail closed or report no
  evidence, and the whole PE parser is strict x64/PE32+ with bounds
  checks. The C01–C09 negative matrix and a real toolchain fixture pair
  (`/CETCOMPAT` vs `/CETCOMPAT:NO` from the same linker) pin the behavior.
- **Build infrastructure**: the VS 18 toolchain on the build host stopped
  honoring `VSLANG=1033` (localized GBK `/showIncludes` output only);
  assigning the captured prefix as a plain variable made CMake 4.3's Ninja
  generator silently drop `CMakeFiles/rules.ninja`. The prefix is now
  stored through the cache-variable path and dependency tracking was
  re-verified by a touch test.

## Automated evidence for rc.4

Executed 2026-10-13, Windows 11 x64, MSVC 19.51 (toolchain 14.51),
CMake 4.3.1, static CRT, `/W4 /WX /utf-8 /permissive- /sdl /guard:cf`,
CETCOMPAT link. Full CTest: **11/11 suites, 0 failures**
(foundation, app command line, TUI, engine core/platform/launch/CLI/
session, verifier fixtures incl. the WinTrust lifecycle matrix with real
WinVerifyTrust positive/negative controls, release input guards incl.
signature-metadata extraction, CET toolchain fixture pair). The
console-event tests require a process tree without the inherited
Ctrl+C-disabled state: agent harnesses set it and every descendant
inherits it, silently suppressing CTRL_C_EVENT handler delivery (a minimal
reproducer confirmed that `SetConsoleCtrlHandler(NULL, FALSE)` in the
child restores delivery). Run such gates from a normal interactive console
or via a scheduled task outside the harness tree; that is an
execution-context property, not a product regression (the 0.7.0-rc.3-era
binary behaves identically under such a tree).

Note on history: the rc.3 release artifacts were not retained in this
workspace (`dist/releases` was empty), so the post-fix read-only recheck of
the rc.3 package has no local object to verify; nothing about rc.3 was
modified or re-marked.

## Open acceptance items (not run — do not present as guarantees)

A01 (manual, real-machine use of the final EXE): Explorer double-click,
Windows Terminal/conhost attended resize and paste, real Desktop
launch receipts (created/reused), real Codex Home authorize/revoke/port
change, real shared-daemon repair, Ctrl+C/window-close during a live
operation, diagnostics copy-to-Notepad. Evidence collection:
`scripts/collect-release-evidence.ps1` (binds facts to the final EXE
SHA-256; read-only).

A02 (distribution): Defender/SmartScreen/SAC classification of this exact
EXE on an affected machine, at least one third-party engine, sample
submission records. Requires the final signed build first. Blocked on a
code-signing certificate; until then every artifact is
UNSIGNED DEVELOPMENT BUILD, not for public distribution.
