# Release checklist (no-Qt single EXE)

Public signed distribution requires every applicable gate. Source integration
and unsigned development packaging may proceed with real-machine/AV items
explicitly recorded as NOT RUN; they must not be presented as release guarantees.

## Build and tests

- [ ] Run `.\scripts\test-noqt.ps1` (dev build, all doctest suites:
      foundation, app command parsing/control, TUI, engine core/platform/
      launch/CLI/session, verifier fixtures and release input guards).
- [ ] Confirm `git diff --check` is clean and the tree is clean.
- [ ] Run canonical `.\scripts\build-portable.cmd -Sign -CertificateSha1 <thumbprint>`
      from the clean pushed commit (refuses a dirty tree). Development
      builds may pass `-AllowUnsignedDev` and are marked unsigned everywhere.
- [ ] Check release tests, embedded provenance (`build-info`),
      plain-console smoke with an isolated temporary config, full `licenses` output, and the import scan
      (no Qt6*/msvcp140*/vcruntime140*/concrt140*/unexpected third-party DLL;
      console subsystem; static CRT).

## Package verification

- [ ] Verify `scripts/verify-package.py --schema static-single`: single-EXE
      transport ZIP (EXE only), sidecar digests, SHA256SUMS linkage,
      release-manifest consistency (qt=false, powershell_appx_discovery=false,
      subsystem=console, crt=static), embedded build-info obtained by
      executing the packaged executable, PE subsystem byte and import scan.
- [ ] Confirm the source ZIP digest agrees across the sidecar,
      `manifest.source_zip.sha256` and SHA256SUMS.txt, contains no build
      products (`*.obj`/`*.exe`/`*.dll`/`*.pdb`/`*.lib`/`*.ilk`/`*.zip`/
      `*.7z`/`*.pfx`/`*.key`) and no `target/`, `dist/` or `.git/` members,
      and that the size report is present. Confirm archive members match the
      fixed commit, including tracked dotfiles, and build inputs came from that archive.
- [ ] Confirm the release directory holds exactly the manifest's artifact
      set (eight files including both sidecars); no stale extras.
- [ ] Confirm the PE mitigation gate: CFG instrumented with the FID table
      present, DYNAMICBASE/HIGHENTROPYVA/NXCOMPAT and CET recorded in the
      manifest and re-derived from the image by the verifier.
- [ ] Confirm Authenticode: signed releases record `authenticode_status=Valid`,
      are independently re-validated through WinVerifyTrust by the verifier,
      and nothing touches the EXE after signing; unsigned releases are
      explicitly marked `UNSIGNED DEVELOPMENT BUILD`.
- [ ] Confirm the destination `dist/releases/<version>-<commit>` did not
      exist before the build (promotion is immutable; the build refuses to
      overwrite) and that promotion was one atomic move from unique staging.

## Acceptance (recorded in docs/TUI_ACCEPTANCE.md)

- [ ] Windows Terminal and legacy conhost: TUI renders, resizes, restores the
      console state on exit (including Ctrl+C and window close).
- [ ] PowerShell and cmd launches; Explorer double-click opens a console.
- [ ] Chinese/space directory and renamed EXE stay fully functional.
- [ ] Real-machine matrix: normal launch receipt (created/reused), backend
      authorization prepare/revoke, repair flow — compared against the
      0.6.1-rc.1 Qt release on the same Desktop version.
- [ ] AV evidence: Microsoft Defender quick scan, SmartScreen download/run
      exercise, at least one third-party engine, one machine that never ran
      Guard; record signed state, detection name (if any), action,
      timestamp, SHA-256. Submit false positives through the vendor channel
      (Microsoft sample submission portal for Defender); never try to evade
      detection in code.

## Publication

- [ ] Promote to `dist/releases/<version>-<commit>/` (the build script does
      this after independent re-verification) and announce the SHA-256.
- [ ] For an unsigned development build, state prominently that it is not
      for public distribution.
