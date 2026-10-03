# Release checklist — single executable

- [ ] Review `git diff --check`, config/consent scope, activation boundaries and dependency changes.
- [ ] Run `scripts/test-cpp.ps1` (dynamic SDK development build, all suites).
- [ ] Run `scripts/test-cpp.ps1 -StaticQt -QtRoot target\qt-static\<key>\install -BuildDirectory target\cpp-static-dev` (static SDK, all suites incl. offscreen-import tests).
- [ ] Check CodeGraph status if installed; use current source when restructuring makes the index stale.
- [ ] Verify normal launch tests never resolve CLI or call daemon stop.
- [ ] Verify repair tests prepare the authorized block before exactly one public stop; failure/cancel prevents later activation.
- [ ] Verify duplicate JSON keys, invalid UTF-8, input/output overflow, unknown fields, timeout, helper cleanup and stale tokens fail closed.
- [ ] Verify `.env` byte preservation, export-variable conflicts, edited-block rejection, revoke failure and config read/write exclusion.
- [ ] Verify worker identity/creation/elevation/early-exit classifications and no bare-EXE fallback for registered applications.
- [ ] Verify role classification tests (values containing role words, `--`, contradictory modes) and that headless roles never create a QPA platform.
- [ ] Verify the static Qt cache: exactly one completed `target\qt-static\<recipe-key>` with matching source digest; `scripts/build-qt-static.ps1` reuses it.
- [ ] Synchronize README, architecture, security, protocol, notices, rebuild guide and acceptance records.
- [ ] Commit in stages and push the intended branch.
- [ ] Run canonical `scripts\build-portable.cmd -StaticQt` from the clean pushed commit (refuses a dirty tree).
- [ ] Check release tests, embedded provenance (both `build-info` spellings), isolated-PATH role smokes, bridge hello/shutdown with stdin held open, malformed-worker rejection, full `licenses` output, and the dumpbin import scan (no Qt6*/msvcp140*/vcruntime140*/concrt140*/unexpected third-party DLL).
- [ ] Verify staging → promotion: `dist/releases/<version>-<commit>` created only after every gate; a failed run leaves no partial release and never overwrites an existing one.
- [ ] Verify `scripts/verify-package.py --schema static-single`: single-EXE transport ZIP, sidecar digests, release-manifest linkage, embedded build-info obtained by executing the extracted executable, compliance recipe key, upstream Qt source digest, every compliance member hash.
- [ ] Confirm the runtime ZIP contains only `CodexProxyGuard.exe`; all notices are embedded (103 notice sections at 0.6.1-rc.1).
- [ ] Confirm the source-compliance archive contains the app snapshot (exact commit), upstream Qt source, static recipe, toolchain record, licenses, REBUILD.md, SOURCE_REVISION.txt and SOURCE_ACCESS.txt; verify its availability alongside the runtime archive before announcing publication (`publication_pending` until then).
- [ ] Optionally rebuild from the compliance archive per `docs/STATIC_QT_REBUILD.md` (unmodified rebuild and one modified-Qt rebuild) before sealing a release series.

Live launch, shared-daemon repair, clean Windows VM loading (no Qt/MSVC/VC redist
installed), Windows 10, high DPI, IME and screen-reader checks are separate
acceptance activities. Record which actually ran; a unit fixture or package
smoke must never be reported as live network or clean-machine verification. Do
not publish a release or sign artifacts implicitly. The transitional dynamic
split package (`scripts\build-portable.cmd` without `-StaticQt`) keeps its own
legacy checklist entries above where they still apply.
