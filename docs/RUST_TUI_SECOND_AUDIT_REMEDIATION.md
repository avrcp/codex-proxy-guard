# Second static audit — remediation record (2026-10-17)

Audit input: `CPG_RUST_TUI_SECOND_AUDIT_ZCODE.md` (remote static review of
`main@9ffff428`). Remediation branch: `fix/rust-tui-audit-20261017`.
Every fix below landed as an atomic commit with a regression test that
failed on the pre-fix code (or a fixture suite for the release tooling).

Statuses: `FIXED` (fix + regression evidence), `DEFERRED_WITH_REASON`.
`Reproduced?` distinguishes what was actually observed on this machine
from what was proven statically.

## Finding status

| ID | Status | Reproduced? | Fix commit(s) | Regression evidence |
|---|---|---|---|---|
| F01 cancellation token reuse | FIXED | yes — all 4 tests failed pre-fix (`LAUNCH_CANCELLED` leaked into every later operation) | `59e555a` (tests) + `d5c43be` (fix) | `cpg-engine` session tests: `cancelled_launch_settles_then_refresh_succeeds`, `timeout_settles_then_next_launch_is_not_pre_cancelled`, `cancel_one_operation_does_not_cancel_following_operation`, `old_worker_cannot_cancel_new_operation` (C01/C02) |
| F02 unbounded helper cleanup | FIXED | yes — stuck-pipe fixtures took 25.0 s pre-fix, ≤5.1 s post-fix (bounded by the helper's own 5 s timeout; cleanup is milliseconds) | `a203749` (tests) + `f5b0650` (fix) | `cpg-windows` `stuck_pipe_descendant_cleanup_is_bounded`, `cancel_during_stuck_pipe_cleanup_is_bounded` (C03); existing timeout/output/cancel suites cover C04/C05 semantics |
| F03 IPv6 Debug underflow | FIXED | yes — `catch_unwind` case panicked pre-fix | `33446ca` | `cpg-domain` `ipv6_overfull_expansion_is_rejected_without_panicking` (C06) |
| F04 TUI console-control scope | FIXED | static (handler never constructed on the TUI path) | `9662448` | code-path inspection + existing console-mode tests; real WM_CLOSE behavior is `MANUAL_REQUIRED` (raising CTRL_CLOSE against the test runner is not survivable) |
| F05 alternate-screen override | FIXED | static (double init provable from `ratatui::init()` contract) | `7dc2b95` | `never` now initializes through `try_init_with_options` (raw mode only); `always` through `try_init`; restore is symmetric (C07/C08/C09 structure). Live terminal matrix re-run below |
| F06 SHA256SUMS before signing | FIXED | static (order provable from script) | `72146b9` | build script now signs → re-queries the signature independently → hashes the final artifacts; verified on the new dev build below (C10) |
| F07 weak promotion gate | FIXED | fixture-proven | `72146b9` | `scripts/test-verify-package-rust.py`: intact synthetic package verifies, 10 tampered variants rejected (C11/C12); canonical build now runs the verifier before promotion |
| F08 YES over clipped scope | FIXED | gate-matrix + model-level tests | `e0030ea` | `cpg-tui` `confirmation_gate_requires_fullly_visible_scope`, `typed_yes_authorizes_only_when_scope_is_visible`, `shrink_after_typing_blocks_authorization` (C13) |
| F09 worker input budget | FIXED | fixture-proven (idle open pipe ignores old check) | `7336a7d` | `cpg-windows` `idle_open_pipe_times_out_instead_of_hanging`, `pipe_data_then_eof_is_collected`, `byte_limit_is_enforced` (C14) |
| F10 editor keys / paste | FIXED | static (dead key arms) | `867f4f4` | `cpg-tui` `editor_supports_cursor_editing`, `editor_paste_is_single_line_and_gate_aware` (C15) |
| F11 idle 50 fps redraw | FIXED | static (draw was first statement of every loop) | `2058a2c` | by construction: draws happen only in the dirty branch; idle loops with no events perform zero draws (C16) |
| F12 plain-mode close during blocking read | DEFERRED_WITH_REASON | not reproduced | — | see below |

## Deferred: F12

`plain::interact` blocks in `read_line` and can only observe a latched
close flag after the read returns. Reproducing the real failure requires
delivering `CTRL_CLOSE_EVENT`/`CTRL_BREAK_EVENT` to a console owned by
the test process: raising close against the shared test-runner console
terminates the runner, and break is intercepted by the raw-mode console
of the TUI path, not the plain path under test. No automated fixture on
this machine can trigger the real signal, and the audit forbids fixing
unreproduced R-items. The narrow exposure (plain mode + redirected stdin
that never reaches EOF + window close) is documented here and in
`docs/TROUBLESHOOTING.md` terms; reopening requires a dedicated child
console fixture.

## F02 semantics after the fix

Reader/writer threads register their real native thread handles
(duplicated from `GetCurrentThread`); settling issues
`CancelSynchronousIo` when aborting and re-issues it every 250 ms while
any thread remains, so pipes held open by descendants of the owned
helper no longer park a `ReadFile`/`WriteFile` past the settle budget.
The in-flight call aborts with `ERROR_OPERATION_ABORTED`, which the I/O
loops treat as a stop once `stopping` is set. A kernel/driver that
refuses the cancellation still cannot produce a success verdict: the
runner retains ownership, records `HELPER_CLEANUP_FAILED` (or
`APPX_ACTIVATION_OUTCOME_UNKNOWN` for a submitted activation) and never
detaches threads — that extreme case is a hard limit, not a bounded
promise, exactly as the audit allows. `waitfor` grandchild fixtures
cover the "silent descendant holds the pipe write end" shape; the
writer variant cannot be driven through the public API because the
16 KiB request ceiling fits the pipe buffer, and the writer uses the
same cancellation mechanism as the readers.

## Release integrity after F06/F07

Order is now: package → sign (optional) → independent
`Get-AuthenticodeSignature` query → `write-rust-manifest.py` (digests
and PE mitigation facts over the final artifacts) → full
`verify-package.py --schema rust-static-single` run (PE gates,
provenance, source binding, SHA256SUMS, artifact set, offline CLI smoke,
independent `WinVerifyTrust` for signed builds) → immutable promotion.
Any verifier failure leaves nothing promoted. The unsigned dev path is
unchanged and remains `public_release_ready: BLOCKED` without a signing
credential — no signed acceptance was simulated or claimed.

`RUST_TUI_MIGRATION_ACCEPTANCE.md` previously recorded the `fc3eaf62`
release EXE with `build-info.commit = 466fd81e…`. The archived
`build-info.json` (and its SHA256SUMS entry) prove the artifact was
always `fc3eaf62c793ce7b40ccb7850f0b0be32af14f57` — the mismatch was a
doc transcription error, not a provenance defect; the doc is corrected.

## Architecture debt recorded (not remediated, per audit scope)

- `cpg-engine/src/launch.rs` still performs `CreateFileW`/`CreateProcessW`
  and `cpg-engine/src/session.rs` still links `ProcessPrng` via FFI, so
  the "every unsafe call lives in cpg-windows" boundary is aspirational
  there. Touching them is deferred until a fix requires it; extract thin
  `cpg-windows` wrappers at that point, not in bulk.
- `Model::notice` is set in several paths but not rendered by the TUI;
  harmless dead state until a future surface uses it.

## Verification runs (this machine, Windows 11 x64)

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`: clean.
- `cargo test --workspace`: 105 passed, 0 failed (was 89 at the audit
  baseline; +16 new regression tests).
- `python scripts/test-verify-package-rust.py`: 11/11 (intact + 10
  negatives).
- `.\scripts\test-rust.ps1`: ALL RUST GATES PASSED (final run).
- Canonical unsigned dev build from the clean pushed branch
  (`4141a07d`): produced
  `dist/releases/CodexProxyGuard-0.8.0-rc.1-rust-4141a07d/` —
  EXE SHA256 `4909dd75861a1c5a2465c832395fb27c3cf2f1ad6f0056229cb8cc4968704a63`
  (1,217,024 bytes), promotion happened only after the
  `rust-static-single` verifier passed, and the promoted immutable
  directory re-verifies independently (PE mitigations, provenance
  `4141a07d…` clean/Rust, source binding, sums, CLI smoke).
  Two verifier findings surfaced during the build runs and were fixed
  before any promotion: the retired C++ license-marker spelling (would
  have failed every real licenses smoke) and the missing
  `BCRYPTPRIMITIVES.DLL` system-import allowlist entry — the latter also
  proves the old fc3eaf62 artifact would not have passed this gate as
  shipped, which the earlier manual import scan had not caught.

Manual/`MANUAL_REQUIRED` items (real WM_CLOSE, Windows Terminal live
matrix, narrow-window visual check) were exercised as live smoke by the
operator model during development of this branch but are not captured as
automated evidence; they are not counted as test passes above.

## Boundary statement

Proxy arguments delivered to a newly activated Desktop process and the
consented `.env` block are file/process facts, not evidence that
end-to-end network traffic actually traverses the proxy; this round of
tests does not verify network effect, commercial signing chains, or
antivirus acceptance.
