# Package identity launch acceptance (2026-09-26)

## Delivery plan and progress

| Stage | Scope | Status |
| --- | --- | --- |
| T0 | Inspect the installed package, manifest entry, running process identity, and prior artifact | Completed, read-only |
| T1 | Preserve registered package identity through discovery and domain state | Implemented |
| T2 | Build a bounded, one-shot package-context candidate and validate it with a controlled fixture | Implemented; actual Desktop pending |
| T3 | Query the exact target process identity and distinguish creation from acceptance | Implemented; actual Desktop pending |
| T4 | Integrate the candidate into CLI/TUI with explicit selection and fail-closed normal launch | Implemented |
| T5 | Run focused regressions, repository gates, and real A/B/C acceptance | Code checks passed; A/B/C NOT RUN at user request |
| T6 | Synchronize docs and deliver a canonical portable artifact from the pushed commit | Docs synchronized; Git and artifact evidence belongs to the delivery record |

## Result

This release is a **code and build candidate**. The user requested no real
Desktop launch during this round. The original package-identity popup is not
claimed fixed, and no ChatGPT/Codex network or sandbox behavior is claimed
verified. The registered package path now fails closed on ordinary Enter/L;
the one-shot `P` or `launch --package-context-compat` candidate requires an
explicit selection.

## Observed local baseline

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

## Controlled fixture evidence

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

## Required A/B/C acceptance

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
