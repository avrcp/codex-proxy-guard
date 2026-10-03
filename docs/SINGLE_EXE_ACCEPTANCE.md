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
