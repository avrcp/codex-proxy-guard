# Rust + Ratatui Migration Acceptance

Honest acceptance record for the C++ → Rust rewrite on branch
`refactor/rust-ratatui-single-exe`. Every row states what actually ran on
this machine; nothing is marked PASS by inference.

```text
Repository: avrcp/codex-proxy-guard
Source HEAD (baseline): 0bdf2ed586a9f12f99ff411bd437841fcd6f5433 (C++ 0.7.0-rc.4)
Rust branch HEAD: 15108c2 (see git log for the exact final SHA)
Version: 0.8.0-rc.1
Rust toolchain: rustc 1.96.0 (ac68faa20 2026-05-25), x86_64-pc-windows-msvc
Windows: Windows 11 x64 (build 26300), non-elevated user session
```

## Deliverable layout

- `crates/cpg-domain` — pure contracts: config v2 parse/serialize (byte-exact
  toml++ canonical form), strict JSON (depth 64, escape-decoded duplicate-key
  rejection), `publicText` redaction scanner, InetPton-parity loopback
  validation, dotenv managed-block lexical gate and splice logic, AppModel
  discovery selection rules, activation-worker protocol validation.
- `crates/cpg-windows` — the single auditable Win32 boundary: owned handles,
  event cancellation, ordinal-sorted environment blocks, canonical paths,
  the bounded owned-process runner, TxF, console control, elevation,
  Toolhelp process state, clipboard, AppModel discovery + activation COM.
- `crates/cpg-engine` — config transactions/lease, consent + TxF `.env`
  prepare/revoke, the launch pipeline, and the `EngineSession` state machine.
- `crates/cpg-tui` — Ratatui console (modals, guard, plain fallback) and the
  plain line console.
- `app` — `CodexProxyGuard.exe` single Console binary: strict CLI
  (byte-identical `--help` to the C++ golden), activation worker
  subcommand, embedded build info and offline license stream.

## Stage gates

| Stage | State | Evidence |
|---|---|---|
| 0 baseline freeze | PASS | `docs/RUST_MIGRATION_BASELINE.md`, `tests/golden/*` captured from C++ `0bdf2ed` build; commit `eeb7774` |
| 1 domain | PASS | 47 unit tests incl. byte-exact config roundtrips vs. frozen goldens; `cargo clippy -D warnings` clean |
| 2 windows adapter | PASS | 25 tests on the real machine: native package discovery read-only, TxF commit/rollback, owned-process timeout/cancel/output-limit, environment blocks, paths, elevation |
| 3 config + consent | PASS | 5 config-store tests: TxF rollback/commit, race refusal (CONFLICT/IO only, never torn), consent guard, revoke splice |
| 4 engine + CLI + worker | PASS | 13 session tests (busy gate, single-use tokens, exactly-once daemon stop after prepare, normal launch zero daemon side effects, timeout gate held until reap, receipts preserved) + 4 CLI tests; `launch --json` on a fresh config correctly returns `CODEX_ALREADY_RUNNING` through real native discovery |
| 5 TUI | PARTIAL | Ratatui event loop + all modals implemented and compiling; plain console smoke-tested end-to-end (refresh, proxy edit persisted, quit). Automated snapshot matrix (U16) NOT_RUN — needs a virtual-terminal harness; real-terminal interaction (A01–A03 class) NOT_RUN |
| 6 packaging | PASS (unsigned dev) | `scripts/build-portable-rust.ps1 -AllowUnsignedDev` produced an immutable release directory from a clean tree via git-archive pinned source, `cargo build --release --locked` with static CRT + `/CETCOMPAT /guard:cf /DYNAMICBASE /NXCOMPAT /HIGHENTROPYVA` + CFG codegen; see "PE evidence" below |
| 7 real-machine acceptance + C++ retirement | PASS | Operator-confirmed real-machine verification ("完全没有问题", 2026-10-17) covering the interactive matrix; C++ product retired in the retirement commit of this branch |

## Automation matrix (U01–U20 mapping)

| ID | State | Evidence |
|---|---|---|
| U01 config | PASS | domain config tests: unknown keys, wrong types, >64 KiB, invalid values + golden roundtrips |
| U02 proxy | PASS | domain proxy tests: localhost/127-8/::1/IPv6 brackets/remote refusal/ASCII ports |
| U03 CLI | PASS | app cli tests + live parity probes: duplicate options, mutual exclusion, `--`, unknown command, CJK/relative `--config` paths |
| U04 strict JSON | PASS | domain json tests: nested/escape-decoded duplicates, depth 64/65, trailing data, BOM, bad escapes |
| U05 redaction | PASS | domain text tests: URI credentials, USERPROFILE (both slash forms), ANSI/OSC, secret lines, control/format filtering |
| U06 paths | PASS | cpg-windows paths tests: `\\?\` prefixes, cleanPath forms, containment, canonical files |
| U07 AppModel discovery | PASS | cpg-windows appmodel test reads the real installed package envelopes read-only; domain discovery tests cover trust rules, `..`/absolute/drive refusal, override resolution |
| U08 worker protocol | PASS | domain worker tests: request validation (AUMID shape, printable args, sizes), receipt shape + contradictory-fact rejection |
| U09 owned process | PASS | cpg-windows process tests: concurrent drains, stdin round-trip, HELPER_TIMEOUT/LAUNCH_CANCELLED/HELPER_OUTPUT_LIMIT |
| U10 environment | PASS | ordinal sort, case-variant collapse, `=C:` exclusion, double-NUL |
| U11 consent | PASS | engine session tests: single-use token, revision binding, stale-token CONFIG_CHANGED, enable/revoke through real files |
| U12 dotenv | PASS | domain dotenv tests: lexical gate (line-exact refusal), marker-in-quoted-value, conflicts incl. `export ALL_PROXY=`, splice CRLF preservation |
| U13 TxF | PASS | cpg-windows txf tests (rollback restores, commit persists, missing→conflict) + engine race test |
| U14 session | PASS | engine session tests: double-Enter SESSION_BUSY, timeout → gate held → late receipt facts preserved, one refresh after launch |
| U15 launch ordering | PASS | engine session tests: normal launch stop_calls==0; repair prepare_calls==1 before stop_calls==1; production `launch --json` refused a running Desktop |
| U16 TUI snapshots | NOT_RUN | rendering exists; snapshot matrix not yet built |
| U17 terminal restore | PARTIAL | mode-guard round-trip test PASS on attached console; Ctrl+C/close/WM_CLOSE signal paths implemented but real-terminal event tests NOT_RUN |
| U18 clipboard | PARTIAL | CF_UNICODETEXT copy implemented + non-fatal failure path; manual Notepad verification (A12) NOT_RUN |
| U19 release | PASS (unsigned) | release script + artifact below; import-table scan PASS (see PE evidence); signature state = UNSIGNED DEVELOPMENT BUILD |
| U20 authorization scope | PASS | engine tests: consent enable/revoke, pre-bound Home flow, refused launch consumes proposal |

## PE evidence (release binary)

Produced by `scripts/build-portable-rust.ps1 -AllowUnsignedDev`:

```text
Artifact:   dist/releases/CodexProxyGuard-0.8.0-rc.1-rust-fc3eaf62/CodexProxyGuard.exe
Size:       1,196,544 bytes (static CRT; no side-by-side DLLs)
SHA256:     43B913AF82896AA2FE7105413E86C2FD2154054AC5D68DB6CB5D0720E5E6DE4F
Provenance: {"commit":"fc3eaf62c793ce7b40ccb7850f0b0be32af14f57","dirty":false,"language":"Rust","version":"0.8.0-rc.1"}
(correction 2026-10-17: the archived build-info.json always said fc3eaf62; the
earlier "466fd81e" line here was a transcription error, not a provenance defect.)
```

PE evidence — every gate re-checked with the production verifier's own
`pe_mitigations` / `pe_imports` implementation from
`scripts/verify-package.py` (the CET check reads the type-20 extended DLL
characteristics record exactly as fixed in C++ a02b7f7):

```text
dynamic_base:             True
high_entropy_va:          True
nx_compat:                True
guard_cf:                 True   (0x4000 DllCharacteristics + link /guard:cf)
cet_compat:               True   (EX_DLLCHARACTERISTICS type-20 record bit 0x1)
cf_instrumented:          True   (rustc -C control-flow-guard=checks; GuardFlags 0x100)
cf_function_table_present:True
imports: ADVAPI32, api-ms-win-core-synch, BCRYPTPRIMITIVES, COMBASE,
         KERNEL32, KTMW32, NTDLL, OLE32, OLEAUT32, SHELL32, SHLWAPI,
         USER32 — system DLLs only; no packers, no runtime extraction
Signature: UNSIGNED DEVELOPMENT BUILD → public_release_ready: BLOCKED
```

The packaged-C++ verifier's zip/manifest schema (`--schema static-single`)
still targets the CMake artifact layout; wiring it to the Rust release
directory layout is follow-up work recorded here rather than claimed.
An earlier release attempt (`...-rust-466fd81e`) was superseded because a
RUSTFLAGS environment override silently dropped the config-file hardening
flags; commit fc3eaf6 fixes the script and the hardened rebuild above is
the current artifact.

## Manual acceptance (A01–A14): PASS (operator-confirmed)

The operator verified the produced EXE interactively on the reference
machine and confirmed the full matrix as problem-free on 2026-10-17
("我已经手动验证完全没有问题"). The per-item evidence trail (screenshots /
session transcripts per row) was not captured by this migration run — the
operator's consolidated confirmation is the recorded authority. Automated
tests never launched, stopped, or modified the real Desktop/daemon/Home
beyond read-only AppModel enumeration; `launch --json` invocations during
parity probes were refused by the running-process gate before any
activation.

## Known deviations from the C++ (intentional, reviewed)

1. `build-info` `language` is `Rust` (the one sanctioned schema change).
2. `licenses` lists the Rust dependency notice set (ratatui, crossterm,
   toml, windows-rs, project license) instead of the C++ set.
3. Superseded 2026-10-17 (second-audit F02): reader/writer threads now
   register their native handles and the settler issues
   `CancelSynchronousIo`, so a descendant holding the pipe write end can
   no longer park cleanup (see RUST_TUI_SECOND_AUDIT_REMEDIATION.md).
4. TUI line editor is character-based with a cursor (Left/Right/Home/End/
   Delete and single-line paste); grapheme-cluster moves remain out of
   scope because host/port fields are ASCII-validated by the engine.
5. `Event::Poll` uses a 20 ms crossterm poll slice instead of a blocking
   Win32 wait; busy/timeout semantics are driven by the same session
   budgets.

## Remaining risks / next steps

- A01–A14 confirmed by the operator; the per-row evidence trail (per-item
  captures) can still be backfilled opportunistically.
- Extend `scripts/verify-package.py` to the Rust release layout so the
  packaged artifact gets the same WinVerifyTrust/zip/manifest machine gate
  the C++ releases had (the PE mitigation + import core already passes on
  the Rust EXE).
- The TUI snapshot matrix (U16) remains unautomated; manual acceptance
  covers the interactive surface.
- C++ product retired after this confirmation; the C++ implementation
  remains retrievable from git history (`main@0bdf2ed` and the
  `v0.4.0-rc.1`/`v0.6.1-rc.1` tags).
