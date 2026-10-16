# Rust Track (0.8.x) alongside the C++ product (0.7.x)

This repository currently carries two product implementations:

- **C++20/Win32 (0.7.0-rc.4)** — the shipped product: `foundation/`,
  `backend/`, `tui/`, `app/main.cpp`, CMake. Public release path:
  `scripts/build-portable.cmd`. Untouched by the Rust work.
- **Rust 2024 + Ratatui (0.8.0-rc.1)** — the full rewrite on this branch:
  `crates/{cpg-domain,cpg-windows,cpg-engine,cpg-tui}`, `app/src/*.rs`,
  Cargo workspace. Development gate: `scripts/test-rust.ps1`. Release path:
  `scripts/build-portable-rust.cmd -AllowUnsignedDev` (public releases add
  `-Sign -CertificateSha1 <thumbprint>`).

Status, evidence and the honest PASS/NOT_RUN matrix for the Rust track:
`docs/RUST_TUI_MIGRATION_ACCEPTANCE.md`. Frozen C++ contract goldens:
`docs/RUST_MIGRATION_BASELINE.md` + `tests/golden/` (kept byte-exact
through `git archive` via `.gitattributes`).

The C++ implementation stays the product of record until the Rust build
passes the human real-machine matrix (A01–A14); deletion of the C++
sources is deliberately deferred to that gate.

## Developer quick start (Rust)

```powershell
cargo test --workspace                     # 89 tests
powershell -ExecutionPolicy Bypass -File scripts\test-rust.ps1
cargo run -p codex-proxy-guard -- --help
cargo run -p codex-proxy-guard -- config-path
.\scripts\build-portable-rust.cmd -AllowUnsignedDev
```

Hardening flags live in `.cargo/config.toml`; the release script re-sets
the full RUSTFLAGS set explicitly because an environment RUSTFLAGS
replaces config rustflags (see commit fc3eaf6).
