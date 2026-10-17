# Release readiness — 0.8.0-rc.1 (Rust)

Single place that answers "what state is this candidate actually in". The
four levels are cumulative and must not be collapsed into one another;
automated isolated tests passing is not functional acceptance, and an
unsigned development package passing is not a public release. Statuses
are `YES`, `NO`, `PARTIAL` or `BLOCKED` with the blocking condition named.

| Level | Meaning | Status |
|---|---|---|
| `code_ready` | Rust rewrite complete, all automated gates green | YES — 89/89 workspace tests; fmt + clippy `-D warnings` clean; U01–U15/U20 machine-verified (see `RUST_TUI_MIGRATION_ACCEPTANCE.md`) |
| `dev_artifact_ready` | Unsigned development package from a clean, pushed commit passes structure/provenance/PE gates | YES — `dist/releases/CodexProxyGuard-0.8.0-rc.1-rust-fc3eaf62/` from the canonical script (pinned git-archive source, `--release --locked`, static CRT, full hardening RUSTFLAGS, immutable promotion); PE mitigation + import gates re-derived with the verifier core: all pass |
| `functional_acceptance_ready` | The final EXE has complete real-use acceptance evidence | YES — operator-confirmed real-machine verification of the interactive matrix on 2026-10-17 ("完全没有问题"); consolidated operator statement recorded as the authority in the acceptance document |
| `public_release_ready` | Everything above plus a valid Authenticode signature and distribution/AV acceptance | BLOCKED — no signing credential in this environment; the artifact is marked `UNSIGNED DEVELOPMENT BUILD` |

## Notes

- The superseded first release attempt (`…-rust-466fd81e`) lost CET/CFG
  because an environment RUSTFLAGS replaced the config-file hardening;
  fixed in commit fc3eaf6 and re-verified on the current artifact.
- The packaged-zip/manifest schema gates from the retired C++ verifier
  are not yet wired to the Rust release layout (PE/import core is); the
  follow-up is tracked in `RELEASE_CHECKLIST.md`.
- C++ (0.7.x) retirement completed after the operator confirmation; the
  C++ implementation remains retrievable from git history (`0bdf2ed`).
