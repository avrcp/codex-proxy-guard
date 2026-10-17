# Release checklist (Rust single EXE)

Public signed distribution requires every applicable gate. Source
integration and unsigned development packaging may proceed with
signature/AV items explicitly recorded; they must not be presented as
release guarantees.

## Build and tests

- [ ] Run `.\scripts\test-rust.ps1` (fmt --check, clippy -D warnings,
      all workspace tests, `git diff --check`).
- [ ] Confirm the tree is clean and pushed.
- [ ] Run canonical
      `.\scripts\build-portable-rust.cmd -Sign -CertificateSha1 <thumbprint>`
      from the clean pushed commit (refuses a dirty tree; development
      builds may pass `-AllowUnsignedDev` and are marked unsigned
      everywhere).
- [ ] Check the release-mode test run inside the script, the packaged
      EXE's `build-info` provenance, `--help`/`--version`/`licenses`
      smoke, and plain-console smoke with an isolated temporary config.

## Package verification

- [ ] The release directory under `dist/releases/<version>-rust-<commit>`
      holds exactly the artifact set (EXE, source.zip, SHA256SUMS,
      build-info.json); releases are immutable — the build refuses an
      existing destination.
- [ ] The source ZIP is the pinned `git archive` snapshot of the built
      commit (tracked dotfiles included) and the build inputs came from
      that snapshot, not the working tree.
- [ ] PE mitigation gate (re-derive with `scripts/verify-package.py`'s
      `pe_mitigations`/`pe_imports` core): CFG instrumentation with a
      non-empty FID table, DYNAMICBASE/HIGHENTROPYVA/NXCOMPAT, and CET
      only from the `IMAGE_DEBUG_TYPE_EX_DLLCHARACTERISTICS` (type 20)
      debug record — never the APPCONTAINER header bit.
- [ ] Import scan: system DLLs only; no msvcp140/vcruntime140 (static
      CRT), no third-party runtime DLLs, no packers.
- [ ] Authenticode for signed releases: signtool with SHA-256 file
      digest and RFC 3161 timestamp; independently re-validate before
      distribution. Unsigned releases are marked `UNSIGNED DEVELOPMENT
      BUILD` and `public_release_ready: BLOCKED`.

## Known follow-up (tracked, not claimed)

- Wire `verify-package.py --schema` to the Rust release layout so the
  zip/manifest/SHA linkage gates run end-to-end on the packaged
  artifact (the PE/import core already passes on the Rust EXE).
