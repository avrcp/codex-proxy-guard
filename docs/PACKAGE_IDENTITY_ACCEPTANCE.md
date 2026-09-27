# Package identity acceptance

Sections are split into **Current acceptance** (the rc.4 → rc.5 native
activation architecture) and **Historical experiments** (retired). The old
`Invoke-CommandInDesktopPackage -PreventBreakaway` package-context candidate
is **retired**: it used a debugging context whose token differs from normal
activation, never passed real Desktop acceptance, and must not be
reintroduced or re-tested.

## Current acceptance (rc.4 → rc.5)

Current architecture: schema-versioned APPX discovery envelope
(`resources/appx-discovery.ps1`), native registered-entry activation
(`IApplicationActivationManager::ActivateApplication`, `AO_NONE`, short-lived
Guard worker), layered proxy delivery (validated Chromium activation
arguments plus the default-off, B-authorized Codex Home `.env` block), and a
disk-truth `BackendProxyRuntimeState` shown as the TUI Coverage line. The
acceptance matrix for this architecture lives in
`docs/RELEASE_CHECKLIST.md` (rows A–F).

User-confirmed on the review machine (2026-09-27, rc.4): the TUI discovers
and displays the registered `OpenAI.Codex 26.924.2738.0` (`App` /
`app/ChatGPT.exe` / FullTrust) without `APPX_DISCOVERY_INVALID`, and
ChatGPT Desktop can be launched. Not yet executed or verified: proxy
coverage of real Chat/Codex traffic, the B-authorized `.env` flow on the
real home, port-change sync, and the D-repair flow (see the matrix).

### 2026-09-27 round 2: APPX discovery JSON protocol fix

The TUI refresh blocked with
`APPX_DISCOVERY_INVALID: malformed PowerShell JSON: data did not match any
variant of untagged enum AppxRecords` before any launch logic ran. Root causes
confirmed by running the then-current production script on this machine:

| Finding | Evidence |
| --- | --- |
| Single package emitted a bare JSON object at the top level | reproduced stdout began `{"package_name":"OpenAI.Codex",...}` (no array), which only the untagged enum tolerated |
| Optional manifest attributes serialized as JSON `null` | `"runtime_behavior":null,"trust_level":null` while the Rust model used `String`, which rejects null |
| Default `ConvertTo-Json` depth (2) unsuitable for the nested envelope | applications nesting grew past the default once the fixed envelope wrapped `records` |

Fix (0.4.1-rc.4, discovery layer only — launch/activation/daemon layers
untouched this round): `resources/appx-discovery.ps1` is now the single
production source (`include_str!`; production, Windows integration tests, and
manual diagnosis execute the same file), emitting a fixed
`{"schema_version":1,"records":[...]}` envelope with `ConvertTo-Json -Depth 8`
and strict UTF-8 output; the Rust side parses the envelope struct, models
optional manifest attributes as `Option<String>`, treats empty stdout after
success as `APPX_DISCOVERY_PROTOCOL_INVALID`, refuses unknown schema versions
with `APPX_DISCOVERY_PROTOCOL_UNSUPPORTED`, and fails closed (no bare-EXE or
"not installed" fallback). The TUI gained an `Entry` diagnostics line
(`ApplicationId · manifest executable · runtime`).

Machine evidence after the fix (read-only, no Desktop launched):
`schema_version=1`, 1 record (`OpenAI.Codex 26.924.2738.0`), `applications`
= 2 objects (`App`, `CodexCoreCommandRunner`) with null optional attributes;
the full production pipeline (`discover_desktop_app`) now returns the
registered `App` entry where it previously failed
(`tests/appx_discovery.rs`, 2/2 passed on this machine). Stage-1 manual TUI
confirmation (App/Entry/Process lines, no `APPX_DISCOVERY_INVALID`) and all
stage-2 launch acceptance remain for the user; the 2026-09-27 round-1 and
2026-09-26 NOT RUN rows below are unchanged.

### 2026-09-27 round 1: native application activation remediation

The 2026-09-27 round (`0.4.1-rc.4`) replaced the experimental
`Invoke-CommandInDesktopPackage -PreventBreakaway` candidate with the Windows
**registered-entry native activation** backend, per the native-activation
remediation manual:

- Registered Desktop applications are activated through the OS-provided
  `IApplicationActivationManager::ActivateApplication` with `AO_NONE`, run in a
  short-lived Guard-owned worker process (`internal-activate-package`, one
  bounded stdin request and one bounded stdout receipt — no named pipes or
  nonces). The worker holds no OpenAI package identity and never runs in the
  package context. The `windows` crate supplies the official COM bindings
  (`CLSCTX_LOCAL_SERVER`, per Microsoft's guidance for short-lived launchers).
- Ordinary Enter/L now routes a registered Desktop to that backend directly.
  The `P` key, `package_context_compat`, the `package-helper` subcommand, and
  the named-pipe protocol were **deleted** from the release path.
- The proxy delivery is layered and reported separately: Chromium
  `--proxy-server` / `--proxy-bypass-list` activation arguments (loopback
  entries map one-to-one; non-mappable `no_proxy` entries are actionable
  errors, never silently dropped), plus a default-off, explicitly consented
  `.env` proxy block bound to one authorized Codex Home (`codex.manage_codex_proxy_env`
  + `codex.proxy_env_home`; TUI `B`). Guard's own `CODEX_HOME` is never
  inferred as the Desktop's home.
- The receipt separates activation, instance (created/reused), package
  identity, application identity (AUMID), proxy delivery, backend
  configuration, and target elevation facts; no single boolean claims
  "verified".
- `launch --activation-only` and `codex-proxy-guard build-info` were added for
  identity comparisons and provenance verification. The build script embeds
  and smoke-verifies commit/dirty.

Machine evidence gathered read-only this round (no Desktop launched):

| Item | Observation |
| --- | --- |
| OS | Windows 11 Pro, build 26200 (10.0.26200.0), PowerShell 5.1.26100.9444 |
| Registered package | `OpenAI.Codex 26.924.2738.0`, entries `App` (AUMID `OpenAI.Codex_2p2nqsd0c76g0!App`, `Windows.FullTrustApplication`, executable present) and `CodexCoreCommandRunner` |
| Worker pre-COM rejection | `internal-activate-package` with malformed stdin exits non-zero with `APPX_ACTIVATION_PROTOCOL_INVALID` and activates nothing |

All 2026-09-26 rows below are preserved unchanged; their A/B/C acceptance
remains **NOT RUN**. The 2026-09-27 real-machine acceptance (start-menu
baseline, activation-only comparison, normal Enter, Chromium/backend proxy
verification, port-change comparison, Guard-exit lifetime, sandbox, package
update) was **not run** in this session either — the interactive Desktop
launches belong to the user. The rows in §11.2 of the remediation manual are
the acceptance checklist; do not mark them PASS without their evidence.

### 2026-09-27 round 3 (rc.5): freeze-review hardening

Final-review fixes applied on top of rc.4: fact-accurate TUI wording
("Submits"/"not established by Guard", never "NOT proxied"), the
`Launch`/`Coverage` relabel driven by the real disk state
(`BackendProxyRuntimeState`: pending / current / stale / conflict / invalid /
unavailable), consent now yields `pending` instead of green, a proxy edit
marks the block `stale` until the next launch syncs it, `D` is blocked for
registered targets without the authorized block
(`BACKEND_PROXY_REQUIRED_FOR_REPAIR`), the registered repair order is now
prepare-`.env`-then-stop-daemon (a failed prepare never wastes a daemon stop;
locked by integration test), and `--activation-only` conflicts with
`--refresh-codex-daemon` at both the CLI and the domain layer
(`INVALID_LAUNCH_OPTIONS`).

## Historical experiments — do not reintroduce

The record below documents the retired 2026-09-26 package-context candidate
investigation. It is kept as history and as negative guidance only.

### Delivery plan and progress (2026-09-26 round)

| Stage | Scope | Status |
| --- | --- | --- |
| T0 | Inspect the installed package, manifest entry, running process identity, and prior artifact | Completed, read-only |
| T1 | Preserve registered package identity through discovery and domain state | Implemented |
| T2 | Build a bounded, one-shot package-context candidate and validate it with a controlled fixture | Implemented; actual Desktop pending |
| T3 | Query the exact target process identity and distinguish creation from acceptance | Implemented; actual Desktop pending |
| T4 | Integrate the candidate into CLI/TUI with explicit selection and fail-closed normal launch | Implemented |
| T5 | Run focused regressions, repository gates, and real A/B/C acceptance | Code checks passed; A/B/C NOT RUN at user request |
| T6 | Synchronize docs and deliver a canonical portable artifact from the pushed commit | Docs synchronized; Git and artifact evidence belongs to the delivery record |

### Result

This release is a **code and build candidate**. The user requested no real
Desktop launch during this round. The original package-identity popup is not
claimed fixed, and no ChatGPT/Codex network or sandbox behavior is claimed
verified. The registered package path now fails closed on ordinary Enter/L;
the one-shot `P` or `launch --package-context-compat` candidate requires an
explicit selection.

### Observed local baseline

| Item | Observation |
| --- | --- |
| OS | Windows 11 Pro, build 26200 |
| Source before changes | `aa9009365afa06d3b3f92b6c478c2aa638e536ce`, `0.4.1-rc.2` |
| Existing portable artifact | `0.4.1-rc.2`; build-info says commit `3a284d8edd97c935ee42fc75d06a9bf478d7853a`, `git_dirty=true`; SHA-256 was verified against the EXE, so this was **not** an exact clean-HEAD artifact |
| Registered package | `OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0` |
| Package family | `OpenAI.Codex_2p2nqsd0c76g0` |
| Desktop application | `App`, manifest executable `app/ChatGPT.exe`, `Windows.FullTrustApplication` |
| Other application | `CodexCoreCommandRunner`, manifest executable `app/resources/codex-command-runner.exe`; excluded as Desktop main entry |
| Desktop execution alias | None; the `App` manifest alias is `codex-chrome-native-host.exe`, not a Desktop launcher |
| Package-context command | `Invoke-CommandInDesktopPackage` present |
| Existing processes | Three sampled running `ChatGPT.exe` PIDs had the exact registered PackageFullName via `GetPackageFullName`; they were not relaunched or stopped |

The old portable EXE's `--version` returned `0.4.1-rc.2`. The user's popup
report is the original symptom; this round did not reproduce it or record its
raw Windows error code.

The repository's actual bounded APPX discovery PowerShell script was also run
read-only on this machine. It returned the same package identity and both
Application records above; no Desktop process was created by that check.

### Controlled fixture evidence

The repository's `child-env-probe` was launched in the registered `App`
context. It reads only a small proxy environment allowlist and its own
OS-assigned package identity. This probe is not a normal Desktop launch.

| Fixture scenario | Parent identity | Child identity | Child proxy sent at creation |
| --- | --- | --- | --- |
| Package context, default child behavior | Exact package | Unpackaged | Present |
| Package context, `-PreventBreakaway` | Exact package | Exact package | Present |

The outer PowerShell's test proxy values did **not** arrive automatically in
the initial package-context process. These observations support a helper that
sets proxy variables at final target creation. They do not prove the actual
Desktop, its backend, or its sandbox remains functional with
`-PreventBreakaway`.

### Required A/B/C acceptance

The user chose code and build work only. All rows below are **NOT RUN**.

| Scenario | Status | Required future evidence |
| --- | --- | --- |
| A: normal registered launch | NOT RUN | Actual target PID, PackageFullName, UI, popup state |
| B: old Guard Enter | NOT RUN | Old artifact identity, actual target PID/identity, popup and exit behavior |
| C: one-shot package-context candidate | NOT RUN | Actual target PID and exact identity, proxy behavior with two valid ports, normal UI, a harmless local Codex task, sandbox and process lifetime |

The release package from the canonical script still needs a target-machine
smoke in a path with spaces or Chinese characters, plus a restart after a
Desktop package update. Neither is inferred from unit tests or the fixture.

The candidate uses Microsoft's documented debugging command. Microsoft says
its token differs from normal activation and makes no guarantee about other
behavior. It is limited here to the selected registered FullTrust Desktop and
requires a one-shot user selection. See
[Microsoft's command reference](https://learn.microsoft.com/en-us/powershell/module/appx/invoke-commandindesktoppackage?view=windowsserver2025-ps).
