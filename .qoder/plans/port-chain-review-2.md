# Port-chain review #2 — remediation plan

Scope: defects found by re-reviewing commit 9569348 against the code that
actually runs, not against its commit message. Nothing here rewrites a module
that was shown correct: classes A/B (held transaction + `ConfigLease`) and the
consented Home block stay as they are.

## Group 1 — engine states that are reachable but mislabeled

1. `blocked_reason` branches on the `retained` label, not on `config_invalid`
   (`session.rs:1120`). A configuration Guard *did* read and proved unusable is
   described as "could not read the configuration; refresh", while refresh is
   exactly the action that cannot help and save is the action that can. That
   string is also reused as the `CONFIG_INVALID` message (`session.rs:1156`).
   Fix: branch on `config_invalid` first; keep "could not read" only for genuine
   read/cancel failures.
2. Coverage claims come from a configuration this attempt did not confirm
   (`session.rs:1097-1101`): after a failed read the view says
   `backend_proxy_config: unknown` while still asserting
   `enabled=true, authorized_home=<retained path>`. Fix: publish `enabled` and
   `authorized_home` only for a confirmed read; the retained endpoint stays
   visible because it is display-only, the consent fact does not.
3. `ProxyView::default()` / `LaunchPlanView::default()` give `state == ""`
   (`session.rs:50,62`), so a fresh `SessionView` renders `proxy.source:""` and
   `plan.state:""` — a fifth undocumented state. Fix: hand-written `Default`
   using `unconfirmed` / `unknown`.
4. The headless CLI creates `ConfigLease` + the `%TEMP%` startup lock
   (`launch.rs:682-684`) before `config.validate()` can fail
   (`launch.rs:732`). Fix: validate first (pure), so a deterministic refusal has
   zero side effects; the bypass refusal necessarily stays after discovery
   because only a registered target needs activation arguments.
5. `Transaction::open` recreate branch (`config_store.rs:84-93`) sets
   `created=true`, which skips the `expected` comparison *and* the
   `may_contain_consent` gate, so `update_proxy` can write a brand-new file that
   re-grants `manage_codex_proxy_env` + bound Home from the retained snapshot —
   consent restored with no prompt. Fix: record `created` on the transaction and
   refuse with `CONFIG_CONSENT_REPAIR_REQUIRED` when a recreated file would
   carry consent fields.

## Group 2 — claims that live in the wrong layer

6. The TUI derives the configured-vs-submitted divergence itself
   (`app.rs:232-256`); plain console and CLI print the same receipt with no
   such statement — three frontends, three truths, and the frontend is doing
   engine judgment. Fix: publish `configured_endpoint`, `submitted_endpoint`,
   `endpoint_diverged` on `LaunchOutcome` from the config the launch actually
   used, and render that one fact in all three frontends.

## Group 3 — honesty text that is unreadable or false on screen

7. The right-hand column renders with no wrap (`app.rs:879`) at ~39 cells on an
   80-column console: `PLAN` shows `http://127.0.0.1:10` (a wrong endpoint) and
   PROXY's qualifier shows `· sav`. Fix: wrap the paragraph, and reorder each
   row so the value precedes its qualifier.
8. Below `body.width < 72` the right column is dropped entirely (`app.rs:835`)
   — no PROXY, no PLAN, no COVERAGE, no ELEVATION, no fallback. Fix: render the
   same rows into the single column.
9. The "delivery facts only" caveat is appended even when the launch failed and
   there is no receipt (`app.rs:292`), and the result panel is fixed at 17 rows
   (`app.rs:1113`) while its content is now 13+ wrapped lines; the hint row is
   drawn over the last line — the caveat — and `outcome_scroll` is unclamped.
   Fix: gate the facts and caveat on a real receipt, size the popup to content
   within the terminal, reserve the hint row, clamp scroll.
10. `diagnostic_text` adds `\n`-separated facts and then runs the whole text
    through `public_text`, which maps every whitespace to a space and collapses
    runs (`text.rs:294-315`): the "lines" are one run-on line, and the 4096
    cap truncates the newest facts because `outcome_message` is itself capped at
    4096. Fix: sanitize per line, join with real newlines, cap after joining.
11. `Modal::Consent` with no proposal at render time shows the *repair launch*
    text above `Type YES` (`app.rs:1038`). Fix: per-modal fallback.

## Group 4 — minor consistency

12. Plain console uses `[committed]` where the TUI uses `· saved, not re-read`;
    `ELEVATION` value is offset one column from its siblings; `LAUNCH` prints an
    empty method before discovery. Fix: share one label mapping, align, use `-`.

## Tests and gates

- New engine tests in `crates/cpg-engine/tests/proxy_port_chain.rs` for 1, 2, 5
  and the state labels of 3.
- New/updated `cpg-tui` tests that render at 80 columns and at a narrow width
  and assert the full endpoint and the caveat are present, plus scroll clamp and
  diagnostic line structure (current tests assert at 100 columns, where clipping
  already happens, and echo internal concatenations).
- `scripts/test-rust.ps1`, `git diff --check`, doc sync (README, DESIGN,
  TROUBLESHOOTING, `docs/PROXY_PORT_CHAIN_ACCEPTANCE.md`), commit, push,
  `scripts/build-portable-rust.ps1 -AllowUnsignedDev`, then build-info/SHA and
  the PE mitigation facts of the shipped EXE. Real Desktop A→B and real Codex
  Home stay NOT RUN — they need an explicit operator decision.
