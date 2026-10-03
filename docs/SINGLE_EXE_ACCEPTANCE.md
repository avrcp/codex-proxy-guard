# Single executable acceptance record

Implementation record for the package-slimming and true-single-EXE program
(baseline `main@ea92dbb`, product `0.6.0-rc.1`). All numbers below are
measured on this machine from the actual artifacts; MiB = bytes / 1,048,576.
PASS/FAIL/NOT_RUN/BLOCKED statuses are per item actually executed.

## Baseline (S0)

Starting `main` SHA: `ea92dbb464afb71b5688b3fe856b54e0f8773fb1`; artifact
built from clean `ef159bfeaed5e27de3c86a8081c9edb5fe5df264`.

| Metric | Measured value |
|---|---:|
| Full bundle ZIP | 81,867,874 bytes (78.075 MiB) |
| Directory total | 102,306,713 bytes (97.567 MiB), 131 files |
| Qt source inside ZIP (compressed) | 48,437,184 bytes (46.193 MiB) |
| `vc_redist.x64.exe` | 18,731,856 bytes (17.864 MiB) |
| Runtime-only portion | 53,466,264 bytes (50.989 MiB) |
| Production EXEs | 2 (`CodexProxyGuard.exe` 189,952 + `engine/codex-proxy-guard.exe` 574,976) |
| Duplicated deployment | `engine/Qt6Core.dll` + full app-local CRT set under `engine/` |

Baseline suites: 7/7 CTest PASS. Evidence: `target/packaging-review/baseline/`
(not committed; regenerate with `scripts/report-package-size.py`).
Status: **PASS**.

## Stage results

| Stage | State | Runtime download | Production EXEs | App-local DLLs | Status |
|---|---|---:|---:|---:|---|
| Baseline full bundle | 0.6.0-rc.1 @ ef159bf | 78.075 MiB ZIP | 2 | 15 (+engine duplicates) | superseded |
| S1 dynamic split | 0.6.0-rc.1 @ 5149655 | 31.882 MiB ZIP (127 members) | 2 | 15 | PASS (built & verified) |
| S2 unified dynamic | 0.6.0-rc.1 @ 479ad12 | 31.882 MiB ZIP (114 members) | 1 | 11 | PASS (built & verified) |
| S3/S4 static plugins | (dev build) | EXE 17.408 MiB | 1 | 0 | PASS |
| S5 static single release | 0.6.1-rc.1 @ f6a87e7 (main) | 7.937 MiB ZIP (1 member) | 1 | 0 | **PASS (promoted)** |

Final release promoted to `dist/releases/0.6.1-rc.1-f6a87e76/` (built from the merged, pushed main HEAD):

| Artifact | Bytes | MiB | SHA-256 (prefix) |
|---|---:|---:|---|
| `CodexProxyGuard.exe` | 18,253,824 | 17.408 | `47498864139511f6…` (full value in `SHA256SUMS.txt`) |
| Transport ZIP (EXE only) | 8,322,199 | 7.937 | recorded in sidecar |
| Source-compliance ZIP | 49,044,092 | 46.77 | recorded in sidecar (277 members) |

The EXE lands under the 25 MiB optimization target from the manual (§9.5)
without disabling exceptions, accessibility, IME/font capability or removing
any notice; no aggressive LTCG/MinSizeRel experiment was needed.

## What was verified

- **Suites**: 9/9 CTest PASS on the dynamic SDK and on the static SDK
  (`test-cpp.ps1` / `-StaticQt`), including the new `app_startup_mode` role
  classification tests and `verify_package_fixtures` (11 failure fixtures).
- **Static linkage**: `CPG_STATIC_QT` imported-target type gate; `-MT` present
  in every compile rule (build.ninja evidence); static plugin set pinned via
  `qt_import_plugins(NO_DEFAULT, ...)`; test binaries import only offscreen.
- **Import table**: `dumpbin /DEPENDENTS` on the released EXE lists only
  Windows system DLLs and API sets — no `Qt6*`, `msvcp140*`, `vcruntime140*`,
  `concrt140*`, `vccorlib*` or third-party DLL. PASS.
- **Single-file smoke** (each in a fresh directory, System32-only `PATH`, no
  `QT_QPA_*`/`QT_PLUGIN_PATH`, EXE renamed, path contains CJK characters and
  spaces): `build-info` (both spellings, clean provenance fields),
  `bridge --config` hello/shutdown with stdin held open, malformed
  `internal-activate-package` input rejected nonzero, `licenses` streams all
  103 embedded notice sections, GUI `--smoke-test` opens and closes on the
  real Windows platform plugin. PASS.
- **Headless independence**: `QT_QPA_PLATFORM=guarded-nonexistent-qpa` does not
  affect headless roles (no QApplication constructed). PASS.
- **Release verifier** (`verify-package.py --schema static-single`):
  single-member ZIP, sidecar and manifest digests, embedded build-info
  obtained by executing the extracted EXE under a scratch PATH, recipe-key
  linkage between release manifest and compliance archive, upstream Qt source
  digest, all 277 compliance member hashes. PASS.
- **Dirty-tree gate**: the release profile refused a dirty working tree during
  development (observed, not just asserted). PASS.
- **Promotion safety**: staging under `target/`; release directory created
  only after all gates; refuses to overwrite an existing release. PASS.

## GUI self-relaunch topology

GUI and engine are separate PIDs of the same executable file
(`applicationFilePath()` + `bridge` argument; activation worker likewise).
The bridge child receives the GUI `--config` path (absolutized before
crossing processes). No recursive GUI launch is possible: role classification
routes `bridge` to QCoreApplication before any QApplication exists.

## Offline licenses viewer (2026-10-03 rectification)

Scope: the About dialog told single-EXE users to find licenses "in the
portable folder" — stale for the single-file topology. The rectification adds
an offline, selectable and copyable license entry that reuses the embedded
notice set with no bridge protocol change, no GUI linkage to `guard_engine`
business code and no packaging change.

Implementation (`gui/src/engine/licenses_reader.*`, `gui/src/ui/licenses_dialog.*`,
`gui/src/ui/main_window.cpp`): Help → About now states that complete license
texts are embedded and offers a "Licenses / Third-party notices" entry. The
modeless viewer spawns this executable's verified `licenses` argument
(program/arguments separated, `CREATE_NO_WINDOW`), collects stdout
signal-driven under a 4 MiB cap (>10x the ≈380 KiB measured corpus: 1,087 +
4,248 + 368,834 bytes of embedded files), keeps a 64 KiB stderr tail, applies
10 s start / 30 s total deadlines, decodes the complete byte buffer in one
pass (chunk-split multi-byte characters stay intact) and stops only the child
handle this object holds. Copy is enabled only for a complete Ready text;
Esc/close cancels an in-flight read; repeated entry reuses the one window, so
at most one read is in flight; Try again starts from fresh state.

Verification environment: Windows 11 (10.0.26300), MSVC 14.51 (VS 18),
CMake/Ninja from VS, dynamic Qt 6.8.3 `msvc2022_64` and the pinned static Qt
SDK `target/qt-static/4b51c65c2f89e9b0` (static CRT). Test object: dirty dev
builds from `0fcdb155` plus this rectification (the formal release build is
recorded separately below once promoted). Evidence:
`docs/tmp/licenses-rectification/` (not committed).

| Check | Result | Evidence |
|---|---|---|
| `scripts/test-cpp.ps1` (dynamic) | PASS — 10/10 CTest | dynamic-ctest-summary.log |
| `scripts/test-cpp.ps1 -StaticQt -QtRoot target/qt-static/4b51c65c2f89e9b0/install -BuildDirectory target/cpp-static-dev` | PASS — 10/10 CTest | static-ctest-summary.log |
| L01 content equals the CLI source stream | PASS | `licenses_tests::l01…` (reader equality) + `backend_bridge::licensesCommandStreamsTheEmbeddedNotices` (real-EXE anchor: exit 0, headers, >100 KiB, terminator) |
| L02 empty output + exit 0 is an explicit error | PASS | `licenses_tests::l02…` |
| L03 failed start reported, UI stays responsive | PASS | `licenses_tests::l03…` (signal-driven; no blocking wait in the reader) |
| L04 nonzero exit / crash are failures with bounded stderr | PASS | `licenses_tests::l04…` |
| L05 stdout split inside a UTF-8 character | PASS | `licenses_tests::l05…` (no U+FFFD) |
| L06 output cap and stderr flood stay bounded | PASS | `licenses_tests::l06…` |
| L07 hanging child hits the deadline; only the owned child stops | PASS | `licenses_tests::l07…` |
| L08 repeated entry keeps a single in-flight read | PASS | `licenses_tests::l08RepeatedPresent…` |
| L09 Esc during load cancels and reaps the child | PASS | `licenses_tests::l09…` |
| L10 main-window close during load | PASS by construction | the viewer is a child of MainWindow; its reader destructor reaps the owned child (same teardown pattern as the bridge); automated evidence via L09 |
| L11 retry/reopen uses fresh state | PASS | `licenses_tests::l11…` (reader + dialog) |
| L12 renamed EXE in a CJK/space directory stays headless | PASS | `backend_bridge::renamedExecutableInUnicodeDirectoryStaysHeadless` |
| R01–R04 regressions (bridge offline/timeout/cancel, busy single-foreground-op, confirm dialogs, suites) | PASS | existing `engine_bridge_tests`, `controller_tests`, `protocol_tests` unchanged and green in both profiles |

Manual matrix items (DPI 100/150/200%, large fonts, light/dark, IME,
keyboard, screen reader, 480×400 layout, clean Windows machine) remain
NOT_RUN as listed below; `licenses_tests` run under the offscreen QPA, which
is not a substitute for those checks.

## Not executed (remains NOT_RUN)

| Item | Status | Note |
|---|---|---|
| Clean Windows VM (no Qt/MSVC/VC redist) | NOT_RUN | PATH-isolated smoke is not a clean-machine proof |
| Real Desktop launch / repair / live proxy | NOT_RUN | requires explicit user authorization per AGENTS.md |
| DPI 100/150/200%, IME, light/dark, screen reader | NOT_RUN | manual acceptance on the static plugin set |
| Compliance-archive relocation rebuild | NOT_RUN | steps recorded in docs/STATIC_QT_REBUILD.md and REBUILD.md |
| Modified-Qt rebuild and relink | NOT_RUN | same |
| Public source publication check | NOT_RUN | `publication_pending`; release is local until published |

## Known limitations

- The static profile requires rebuilding the SDK to update Qt (no DLL swap);
  the dynamic split profile remains available for that workflow.
- Byte-identical cross-machine reproducibility is not claimed; only inputs
  and steps are pinned.
- `licenses` output is the embedded notice set (103 sections at 0.6.1-rc.1);
  the Qt SDK binary manifest (`sdk-6.8.3-msvc2022-x64.json`) ships in the
  compliance archive rather than the EXE because it describes the dynamic
  SDK, not the static build.
- The 0.6.0-rc.1 full bundle remains valid historical evidence; its verifier
  schema (`legacy`) is still enforced.
