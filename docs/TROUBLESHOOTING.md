# Troubleshooting

## Configuration and local proxy

Use the TUI proxy editor (`E`) or the plain console `E` for a loopback
HTTP/Mixed host and port. An invalid config blocks launch; unknown TOML
fields are rejected. For configuration changes made by another process,
refresh (`R`) before editing or confirming again. `CONFIG_LOCK_FAILED` can
mean a launch still holds its config lease; finish/cancel it before a
mutation.

The PROXY row shows the endpoint of the last configuration read that
succeeded (or the value Guard just committed, labelled `saved, not re-read`).
When a refresh cannot read the file, that endpoint stays on screen, labelled
`last good read`, next to the error and the `BLOCKED` status: the displayed
port does not fall back to the built-in default. Fix the file and press `R`.
A read failure by itself never licenses Guard to overwrite the path — saving
then fails (`CONFIG_LOCK_FAILED`, `CONFIG_CHANGED`) and nothing is written.
Replacing unusable stored bytes needs the positive `CONFIG_INVALID` evidence
Guard only has after actually reading bytes it cannot use; a file containing
authorization fields is still refused with `CONFIG_CONSENT_REPAIR_REQUIRED`.

`PLAN not buildable · PROXY_BYPASS_UNSUPPORTED` means a stored `no_proxy`
entry has no Chromium equivalent: the activation bypass list accepts
`localhost` and loopback IPv4/IPv6 only. The configuration itself is valid
and stays saved, but a registered launch is refused before the config lease
and before any process side effect, and the `L` action reports the same
reason. Narrowing the list is the only fix, and note it is the same list that
is written into the authorized Codex Home block. The native (non-registered)
path delivers `NO_PROXY` through the process environment instead and is not
affected — the PLAN row then reads `via process environment`.

## Backend proxy block

"Not authorized" is the default. Use `B` and type `YES` against the exact
Home shown in the confirmation modal. The block is prepared during a
registered normal launch, or before any registered repair daemon stop. It
is not a network probe. Existing HTTP_PROXY/HTTPS_PROXY/NO_PROXY/ALL_PROXY
keys outside Guard's block, including export spellings, are conflicts;
Guard never overwrites them.

If revocation fails, the config keeps consent and Home so the operation
stays retryable. Resolve permissions or edited markers/content manually,
refresh and retry. Do not reset the config to hide a failed revoke;
`init-config --force` rejects resetting active consent.

`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED` means the `.env` contains
multi-line quoted values, a continuation or an unterminated quote. Guard
refuses to edit such files at all — no byte changes, the error names the
line, and consent stays bound for retry after you fix the file manually.
This is a lexical safety refusal, not a network claim.

## Repair and the shared daemon

Normal launch has no daemon effects. Repair requires a fresh single-use
`YES` confirmation (CLI `launch --refresh-codex-daemon`, TUI `D`) and can
interrupt shared tasks. Registered repair requires backend proxy
authorization; preparation failure guarantees no stop call. Only the
public `codex app-server daemon stop` command is used. Unsupported CLI,
malformed output, non-zero exit, timeout or cancellation blocks
activation; shared service outcome may be unconfirmed. A confirmed stop
followed by launch failure is reported as such.

## Single executable and the TUI

There is exactly one file and no DLL layout to preserve: copy
`CodexProxyGuard.exe` anywhere (including Chinese/space paths) and rename
it freely. `build-info` exposes the version/commit/dirty provenance. If
the terminal cannot host VT output (or stdin/stdout is redirected and no
command is given), Guard falls back to the line console; `console
--plain` forces it (`--plain` is rejected on headless commands). If the
interactive TUI cannot initialize, it degrades to the plain console
rather than crashing; if that also fails, a redacted
`CONSOLE_OUTPUT_FAILED` line is reported on stderr and the process exits
1 — no crash file is written anywhere.

A foreground operation that exceeds its budget retains that observation
alongside any committed configuration or verified receipt; the UI shows
"Cancelling timed-out operation" while the old task is being reaped and
refuses new operations in that window. Refresh after a timeout instead of
resubmitting.

A submitted activation can remain unknown
(`APPX_ACTIVATION_OUTCOME_UNKNOWN`) even after cancellation — refresh
instead of retrying. Quitting the TUI (`Q`) or closing the window never
terminates Desktop. The proxy editor keeps your input when a save fails;
fix the reported cause and retry, or verify the stored settings after a
mid-save engine failure instead of resubmitting blindly. Copy Diagnostic
(`C` in the result modal) needs a working clipboard; on failure it shows
`CLIPBOARD_FAILED` instead of silently pretending the copy succeeded.

Backend proxy block writes require working local NTFS transactions.
Unsupported filesystems keep consent and refuse modification — there is
no nontransactional fallback (see the architecture note on the filesystem
transaction decision).

## Build and toolchain

The Rust toolchain is pinned by `rust-toolchain.toml` (1.96.0,
`x86_64-pc-windows-msvc`). If a release EXE unexpectedly lacks CET/CFG,
check that the release script's full RUSTFLAGS set was applied: an
environment RUSTFLAGS **replaces** `.cargo/config.toml` rustflags
entirely (this is exactly the fc3eaf6 fix). Re-derive the mitigation
facts with `scripts/verify-package.py`'s `pe_mitigations` before
promoting any artifact.
