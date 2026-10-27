# Proxy Port Chain Acceptance (2026-10-27)

Honest answer to the port-chain review ("改了端口不生效"): which stage can
actually fail on this codebase, what was proven by executing code here, and
what remains unexecuted. Nothing in this file is marked PASS by inference.

```text
Repository: avrcp/codex-proxy-guard   Branch: main   Base HEAD: c2d838f
Version: 0.8.0-rc.1
Toolchain: rustc 1.96.0 x86_64-pc-windows-msvc
Host: Windows 11 x64 (build 26300), non-elevated user session
Automation: crates/cpg-engine/tests/proxy_port_chain.rs + cpg-engine/cpg-tui unit tests
```

No automated test in this record activates a real Desktop, writes a real Codex
Home, or runs a daemon command. Every engine test injects a fixture AUMID
(`OpenAI.Codex_2p2nqsd0c76g0!App`) and an activation seam that only records the
arguments it was handed.

## The four classes, separated

| Class | Question | Verdict on this codebase |
| --- | --- | --- |
| A | Was the edited port ever saved? | Not defective. `update_proxy` writes through a held transaction with expected-content comparison, and a save that fails leaves the stored bytes byte-identical (`failed_save_leaves_the_stored_configuration_untouched`). |
| B | Did the launch use a stale snapshot? | Not defective. The launch re-reads the file and holds a `ConfigLease`; a snapshot that no longer matches the file is refused before any side effect (`a_launch_refuses_a_configuration_that_changed_on_disk`). |
| C | Did the launch carry the right arguments? | Was late and invisible. `proxy_plan` is pure and correct, but an unsatisfiable stored `no_proxy` only failed deep inside the launch and nothing on screen said so. Now a typed view fact, gated early (defect 3 below). |
| D | Did Desktop / the Codex backend adopt it? | Not verifiable by Guard by design — there is no network probe. Guard states the delivery fact, the instance observation, and explicitly that adoption is unconfirmed. |

## Defects fixed (each reproduced by a failing test before the change)

1. **A failed configuration read wiped the displayed configuration.**
   `EngineSession::poll()` published any snapshot result, and `refresh()`
   always returns `snapshot: true` — so after a read failure the view carried
   `TaskResult::default()`: host `127.0.0.1`, port **10808**. An edited port
   looked like it had been reverted exactly when the file became unreadable
   (locked, truncated, replaced by a directory).
   Now `publish()` keeps the last configuration Guard actually read
   (`known_good`) for display, marks it `retained`, and still takes readiness,
   discovery facts and the error from the failed attempt, so `BLOCKED` and the
   error stay visible. Repro tests:
   `a_failed_refresh_retains_the_last_known_good_configuration`,
   `a_committed_save_survives_a_failing_following_refresh`.
2. **"Not ready" was treated as evidence of an invalid file.**
   `request_set_proxy` computed the recovery flag as `!self.state.ready`, so
   any read failure — including a path Guard could not read at all — authorized
   a whole-file rewrite built from whatever the view happened to display,
   silently discarding unrelated fields (bound Home, `no_proxy`). Recovery now
   requires positive `CONFIG_INVALID` evidence from bytes Guard actually read;
   an unreadable path is refused with `CONFIG_LOCK_FAILED` and stays untouched.
   Repro test: `a_failed_read_does_not_offer_an_unsafe_recovery`.
3. **The launch plan was neither a view fact nor an early gate.**
   `proxy_plan` refuses a bypass entry that Chromium cannot express
   (`PROXY_BYPASS_UNSUPPORTED`), while `Config::validate` accepts the same
   stored `no_proxy`. A registered target therefore reached a launch attempt
   that could only fail late. `SessionView` now carries `plan.state`
   (`available` / `environment` / `unavailable` / `unknown`), the planned
   endpoint and the refusal; `can_launch` is gated for registered targets only
   (the native EXE legitimately delivers `NO_PROXY` through the environment),
   and `request_launch` refuses before the config lease with the same error.
   Repro tests: `proxy_plan_rejects_what_the_configuration_may_still_store`,
   `an_unbuildable_plan_blocks_a_registered_launch_before_any_side_effect`,
   `a_native_target_still_launches_on_the_environment_channel`,
   `an_unread_configuration_claims_no_plan`.

## Truthfulness changes in the frontends

- PROXY row annotates where the endpoint came from: `last good read`
  (retained after a read failure) or `saved, not re-read` (committed but not
  re-read); plain console uses the same tags.
- New PLAN row: `via activation arguments · <endpoint>`,
  `via process environment · <endpoint>`, `not buildable · <code>`, or
  `not confirmed · press R`. No delivery claim is rendered for a configuration
  that was not read or a target that was not discovered.
- The launch result panel now lists `Configured` and `Submitted` endpoints,
  the method and the delivery channel, warns when the submitted endpoint
  differs from the current configuration, and ends with "Guard verifies these
  delivery facts only: Desktop adoption and network use are not verified."
- The native path is no longer described as an activation: a created native
  instance reads "Desktop process created with the proxy in its environment
  (…)".
- Copied diagnostic gained `config:`, `proxy: <url> (<source>)`,
  `plan: <state> <endpoint>` and `launch: <method>` lines; it remains bounded
  and redacted, records no environment dump and no secret material.
- `launch` (headless CLI) prints the submitted `endpoint=` and the same
  delivery-facts-only caveat line.

## Matrix T01–T28

"PASS (automated)" means the named test ran on this machine in this session.

| ID | Review item | Result | Evidence |
| --- | --- | --- | --- |
| T01 | default port loads | PASS (automated) | `edited_port_survives_the_file_reload_and_a_new_session` (127.0.0.1:10808, `http://127.0.0.1:10808`) |
| T02 | TUI edits the port and saves | PASS (automated, engine-side) / BLOCKED (keystroke) | save path covered by the same test; the keystroke round trip needs the ConPTY harness — see "ConPTY acceptance status" in `RUST_TUI_MIGRATION_ACCEPTANCE.md` |
| T03 | re-read the TOML after saving | PASS (automated) | `edited_port_survives_the_file_reload_and_a_new_session` |
| T04 | port survives a restart | PASS (automated) | a second `EngineSession` over the same path reads 10809 in the same test |
| T05 | explicit `--config` round trip | PASS (automated) | every chain test drives an explicit absolute path; CLI `--config` parsing is covered by `app/src/cli.rs` tests |
| T06 | failed save keeps the old configuration | PASS (automated) | `failed_save_leaves_the_stored_configuration_untouched` (bytes identical, `CONFIG_INVALID`, `CONFIG_CHANGED` on drift) |
| T07 | save OK, refresh fails → committed fact kept | PASS (automated) | `a_committed_save_survives_a_failing_following_refresh`, `mutation_committed_fact_survives_refresh_failure` |
| T08 | concurrent edit blocks a stale launch | PASS (automated) | `a_launch_refuses_a_configuration_that_changed_on_disk` |
| T09 | port A → correct URL/arguments | PASS (automated) | `proxy_plan_tracks_the_configured_port_for_every_host_shape` |
| T10 | port B → correct URL/arguments | PASS (automated) | same test (10808/10809/7890/1/`::1`) |
| T11 | IPv6 endpoint | PASS (automated) | same test (`http://[::1]:10809`, bypass `[::1]`) |
| T12 | `no_proxy` rules | PASS (automated) | same test, plus `proxy_plan_rejects_what_the_configuration_may_still_store` for the gap |
| T13 | receipt matches this launch | PASS (automated) | `no_launch_path_reuses_another_configuration_than_the_one_it_was_given` |
| T14 | no hardcoded old port | PASS (automated) | same test asserts the submitted arguments contain 10809 and never 10808 |
| T15 | registered arguments reach the activation interface | PASS (automated) | same test inspects the exact string handed to the activation seam |
| T16 | plain EXE environment carries the new port | PASS (automated) | same test (`HTTP_PROXY`/`HTTPS_PROXY`/lower-case, `NO_PROXY`, `ALL_PROXY` removed) |
| T17 | new-instance receipt consistent with PID | PASS (automated, observation layer) | classification is unit-tested in `cpg-windows` (`instance_classification_boundaries`); the panel prints `PID` and `Instance` together. NOT RUN against a real Desktop |
| T18 | reused instance is not reported as adopted | PASS (automated, wording) | `a_reused_or_unknown_instance_is_never_reported_as_adoption` |
| T19 | unknown instance never auto-retries | PASS (automated, wording + engine) | same test; the engine reports unknown and performs no retry (`double_launch_is_refused_while_busy`) |
| T20 | real Desktop adopts the delivered arguments | **NOT RUN** | requires activating the installed `OpenAI.Codex` package on this machine |
| T21 | unauthorized never writes `.env` | PASS (automated) | `home_block_is_absent_until_authorized_and_then_tracks_the_port`, `an_unbound_home_is_refused_without_creating_anything` |
| T22 | authorization creates the managed block | PASS (automated) | same test (`pending` → `current`, exact managed bytes) |
| T23 | port change updates the block | PASS (automated) | same test (`stale` → re-prepare → `current`) |
| T24 | external conflicting variables refuse safely | PASS (automated) | same test (conflict refusal, bytes untouched) |
| T25 | a damaged block is never overwritten | PASS (automated) | same test (`BACKEND_PROXY_BLOCK_INVALID`, unsupported lexical gate) |
| T26 | bytes outside the block are preserved | PASS (automated) | same test (user bytes and `\r\n` survive; idempotent prepare writes nothing) |
| T27 | backend reload verified independently | **NOT RUN** | Guard performs no network or process probe by design; only the user's own backend can confirm this |
| T28 | backend adopts the new port after restart | **NOT RUN** | real Codex Home plus a real backend restart; explicitly out of scope for automation |

## Real-machine A→B acceptance

The review's end-to-end scenario (edit A→B, fully quit Desktop, launch from
Guard, then confirm the traffic path) was **NOT RUN** in this session: it
activates the installed Desktop package and would interrupt whatever the user
has running there. The consented Codex Home block was likewise not exercised
against a real `~/.codex/.env`. Both need an explicit operator decision; the
hermetic chain above is what can be proven without touching them.

## Gates executed

```text
cargo fmt --all -- --check                     clean
cargo clippy --workspace --all-targets --locked -D warnings   clean
cargo test --workspace --locked                124 passed; 0 failed; 3 ignored
git diff --check                               clean
```

The 3 ignored tests are the pre-existing ConPTY TUI harness cases, ignored
because child output does not reach the session pipe on this host — a harness
problem, recorded in `RUST_TUI_MIGRATION_ACCEPTANCE.md`, not a product pass.

## Release package smoke (same session)

The canonical script built an unsigned development package from the committed,
clean tree. Only commands that touch a throwaway `--config` path ran; no real
Codex Home, no Desktop activation, no daemon command.

```text
Artifact: dist/releases/CodexProxyGuard-0.8.0-rc.1-rust-9569348e/CodexProxyGuard.exe
Commit:   9569348eaa820d1bd451488d340ca14a92f697bb   dirty: false
SHA256:   4F4E414B5397E9607CD4CF61DF94428671C4EBE264A32229D62ED923134560EF
Size:     1,207,296 bytes   Signature: UNSIGNED DEVELOPMENT BUILD
PE facts: dynamic_base, high_entropy_va, nx_compat, guard_cf, cf_instrumented,
          cf_function_table_present, cet_compat — all true
```

`--mitigations-of` is the verifier's PE parsing core; the full `--release`
manifest gate still expects the retired C++ package schema and was not run
against this layout (tracked follow-up work, no script changed here). The
release-profile test pass is part of the same script run.

Executed against the packaged EXE:

| Check | Result |
| --- | --- |
| `--version`, `--help`, `build-info`, `licenses` (527 lines, offline) | PASS |
| `init-config --proxy-port 10809` writes the port, `config-path` resolves the explicit file | PASS — class A proven by the shipped binary |
| Plain console over a temp config | `Proxy: http://127.0.0.1:10809 \| Plan: available http://127.0.0.1:10809` |
| `no_proxy = ['localhost','intranet.example']` in the stored file | `Plan: unavailable` plus `PROXY_BYPASS_UNSUPPORTED: …` — defect 3 visible without launching anything |
| Valid config read, then file truncated mid-session and refreshed | `Proxy: http://127.0.0.1:10809 [retained] \| Plan: unknown`, `CONFIG_INVALID: Configuration is not valid TOML.` — defect 1 fixed in the shipped binary; before the change this displayed the default 10808 as if it were configured |
| Real Desktop A→B launch, backend adoption | **NOT RUN** — see above |

## Second remediation pass (same day, after re-review)

The re-review re-read the changed source and confirmed classes A (save) and B
(stale launch snapshot) were already sound. What it found instead was a layering
violation and a set of statements that were louder than the evidence behind
them. Each item below names the test that pins it; every one ran on this
machine in this session.

| # | Defect | Fix | Evidence |
| --- | --- | --- | --- |
| 1 | `ProxyView::default()`/`LaunchPlanView::default()` used `""` as a fifth, undocumented state | explicit `Default` impls: `unconfirmed` proxy state, `unknown` plan state | `a_view_that_has_read_nothing_states_no_empty_state` |
| 2 | A retained snapshot kept claiming backend consent and the bound Home after the read failed | coverage and `authorized_home` are published only for a configuration that was actually read | `a_failed_read_stops_claiming_the_backend_authorization` |
| 3 | One blocked reason covered three different states, telling an unusable-file user to refresh | three sentences: repair-by-saving, refresh-after-read-failure, nothing-read-yet | `an_unusable_configuration_is_offered_as_a_repair_not_a_refresh` |
| 4 | A recovery save could re-create a deleted config carrying consent from a stale snapshot, i.e. re-granting authorization | `Transaction` reports `created`; `update_proxy` refuses before the open and after it, file left absent | `a_recreated_configuration_does_not_restore_backend_consent` |
| 5 | Option conflict and config validation ran after the config lease and the cross-process startup lock were taken | `preflight()` runs first; the bypass-expressibility refusal stays after discovery because only a discovered target proves arguments are needed | `a_deterministic_refusal_needs_no_file_handle_or_process` (pure refusal, no lease, no lock), `an_unbuildable_plan_blocks_a_registered_launch_before_any_side_effect` |
| 6 | The TUI compared the receipt endpoint with the live view and drew its own conclusion | `LaunchOutcome` carries `configured_endpoint`, `submitted_endpoint`, `endpoint_diverged`; the console renders the engine's verdict | `the_result_modal_separates_the_submitted_endpoint_from_the_current_one` |
| 7 | A failed launch inherited the previous receipt's delivery lines and the caveat | facts are receipt-gated: without a receipt the panel states "no delivery receipt recorded", which is also honest for `APPX_ACTIVATION_OUTCOME_UNKNOWN`, plus the retained budget fact | `a_failed_launch_states_no_delivery_fact` |
| 8 | PROXY/PLAN were cut off at 80 columns and dropped entirely on a narrow terminal | both body columns wrap; under 72 columns the facts join the single column; every label keeps its own cell (`ELEVATION` no longer runs into its value) | `an_80_column_terminal_keeps_the_endpoint_and_its_source`, `a_narrow_terminal_still_shows_every_engine_fact` |
| 9 | The result panel was a fixed height, so the last fact — the delivery limitation — sat behind the hint row | panel height derives from its own wrapped content (`result_panel_size`), hint row reserved | `the_result_panel_shows_the_last_fact_without_scrolling` |
| 10 | `public_text` collapses whitespace, so the copied diagnostic was one run-on line where a long path swallowed later fields | one field per line, each redacted on its own, bounded 4096 with an explicit "further line(s) omitted" tail | `the_copied_diagnostic_names_the_file_the_endpoint_and_the_plan` |
| 11 | The consent modal described a live proposal even when the proposal was spent | expired-proposal wording for `Modal::Consent` with no proposal; the live wording still comes from the engine's own proposal and Home | `a_spent_consent_proposal_does_not_describe_a_live_operation` |
| 12 | The three frontends described one receipt three ways | `DELIVERY_LIMITATION`, `DIVERGED_ENDPOINT`, `NO_RECEIPT`, `BUDGET_EXCEEDED`, `dash`, `proxy_source_label` shared from `cpg-tui` and used by the TUI, the line console and the headless CLI | `cargo test -p cpg-tui` plus the shipped-EXE smoke below |

Deliberately **not** changed, after checking them: `known_good` never
authorizes a write (a read failure still refuses recovery writes, and only
positive `CONFIG_INVALID` evidence licenses replacing bytes); the
`may_contain_consent` byte gate; the session-level PLAN gate versus the CLI's
`PROXY_BYPASS_UNSUPPORTED` refusal — the two differ by design, both fail closed,
and the ordering is now documented in `ARCHITECTURE.md` and `SECURITY.md`.
Mutation outcomes keep reporting committed/refresh-error/save-failure states as
before; the engine's `exceeded_budget` fact for a save was already conveyed
through those, and no wording was invented for it here.

## Gates executed (second pass)

```text
cargo fmt --all -- --check                     clean
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo test --workspace                         133 passed; 0 failed; 3 ignored
git diff --check                               clean
.\scripts\test-rust.ps1                        ALL RUST GATES PASSED
```

The release package for this pass, its PE facts and the commands actually run
against it follow; anything not executed is marked NOT RUN rather than inferred.

## Third remediation pass (same day, after a second re-review)

The second re-review read the changed source again, this time asking a narrower
question: is every value on screen a fact, or is some of it a default that Guard
invented? One place still invented a value, and it invented the exact value the
original complaint was about.

| # | Defect | Fix | Evidence |
| --- | --- | --- | --- |
| 1 | `view().proxy` published the built-in endpoint (`http://127.0.0.1:10808`) for a session that had read no file at all. A wrong `--config` path or an unreadable file therefore showed the default port with no qualification — the "my port is back to 10808" reading, manufactured by Guard rather than by the user's file | the engine publishes an endpoint only for a configuration it read, committed or retained; before that the row is empty and the state says `not read yet`. `public_json` reports the same empty endpoint with `source: "unconfirmed"` | `a_session_that_never_read_a_file_shows_no_endpoint` (unread path shows nothing, a real read then shows 10809 with no default involved) |
| 2 | The proxy editor prefilled that invented endpoint, so Enter could save a port the user never typed | the editor opens blank while no endpoint is published | `the_editor_prefills_no_endpoint_that_was_never_read` |
| 3 | The line console prompted `Port [0]` — the zero standing for "unknown" rendered as a digit, and Enter would have submitted it | both prompts stay empty until the engine publishes an endpoint, and an empty draft is refused with the reason instead of a `CONFIG_INVALID` about `0` | keystroke round trip NOT RUN (ConPTY harness still blocked — see `RUST_TUI_MIGRATION_ACCEPTANCE.md`); verified against the packaged EXE in the third-pass smoke below |
| 4 | The consoles decided "is there an endpoint to prefill?" by matching the published state string, so a new state would silently publish a default again | `ProxyView.endpoint_known` is the engine's own fact — published only for `read`, `committed` or `retained` — and both consoles read that field instead of interpreting a label | `the_proxy_row_words_every_published_endpoint_state`, `the_editor_prefills_no_endpoint_that_was_never_read` |
| 5 | A stored file Guard read and could not parse was displayed as `not read yet`, pointing the user at a refresh that reports the same bytes again | a fifth published state `invalid`, worded `stored file not usable`, with no endpoint; a previously read endpoint still wins and stays `last good read` | `an_unusable_stored_file_states_itself_instead_of_a_missing_read` |

The gate is the engine's published fact, not a frontend guess: both consoles ask
`view.proxy.endpoint_known` rather than testing the port for zero, so "no
endpoint" stays one engine fact with one wording in three places.

Not changed in this pass, deliberately: the retained-after-failure endpoint (a
read fact, correctly labelled `last good read`), the `saved, not re-read` fact,
and the launch gate — a session that has read nothing was already unable to
launch, and this pass only stopped the display from implying otherwise.

## Gates executed (third pass)

```text
cargo fmt --all -- --check                     clean (after cargo fmt)
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo test --workspace --locked                137 passed; 0 failed; 3 ignored
git diff --check                               clean
.\scripts\test-rust.ps1                        ALL RUST GATES PASSED
```

The 3 ignored tests remain the ConPTY TUI harness cases; they are not a pass.
