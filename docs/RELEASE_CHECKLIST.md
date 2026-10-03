# Release checklist — C++ engine

- [ ] Review `git diff --check`, config/consent scope, activation boundaries and dependency changes.
- [ ] Run `scripts/test-cpp.ps1` (MSVC warnings-as-errors, all backend/Qt tests).
- [ ] Check CodeGraph status if installed; use current source when migration makes the index stale.
- [ ] Verify normal launch tests never resolve CLI or call daemon stop.
- [ ] Verify repair tests prepare the authorized block before exactly one public stop; failure/cancel prevents later activation.
- [ ] Verify duplicate JSON keys, invalid UTF-8, input/output overflow, unknown fields, timeout, helper cleanup and stale tokens fail closed.
- [ ] Verify `.env` byte preservation, export-variable conflicts, edited-block rejection, revoke failure and config read/write exclusion.
- [ ] Verify worker identity/creation/elevation/early-exit classifications and no bare-EXE fallback for registered applications.
- [ ] Synchronize README, architecture, security, protocol, dependency/license and acceptance records.
- [ ] Commit in stages and push the intended branch.
- [ ] Run canonical `scripts/build-portable.cmd` from the clean pushed commit.
- [ ] Check release tests, packaged GUI/engine provenance, isolated-PATH GUI and hello/shutdown smoke (stdin kept open).
- [ ] Verify ZIP CRC, ZIP SHA-256, every `files_sha256` member, official deployed Qt hashes, and dirty=false. The canonical script runs `scripts/verify-package.py` automatically on clean commits; re-running it by hand is an optional independent confirmation.
- [ ] Confirm package includes engine Qt Core + VC runtime, root GUI dependencies, all notices and corresponding Qt source.

Live launch, shared-daemon repair, clean Windows VM loading, Windows 10, high DPI,
and screen-reader checks are separate acceptance activities. Record which actually
ran; a unit fixture or package smoke must never be reported as live network or
clean-machine verification. Do not publish a release or sign artifacts implicitly.
