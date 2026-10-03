# Troubleshooting — C++ release

## Configuration and local proxy

Use the GUI proxy editor, or console `E`, for a loopback HTTP/Mixed host and port.
An invalid config blocks launch even after an error dialog is dismissed. Unknown
TOML fields are rejected. For configuration changes made by another process,
refresh before editing or confirming again. `CONFIG_LOCK_FAILED` can mean a launch
still holds its config lease; finish/cancel it before a mutation.

## Backend proxy block

“Not authorized” is the default. Use the GUI authorization dialog or console `B`
and confirm the exact Home. The block is prepared during a registered normal
launch, or before any registered repair daemon stop. It is not a network probe.
Existing HTTP_PROXY/HTTPS_PROXY/NO_PROXY/ALL_PROXY keys outside Guard's block,
including export spellings, are conflicts. Guard does not overwrite those keys.

If revocation fails, the config keeps consent and Home so the operation remains
retryable. Resolve permissions or edited markers/content manually, refresh and
retry. Do not reset the config to hide a failed revoke. `init-config --force`
rejects resetting active consent. A physical disk failure may also prevent a
configuration write and rollback; inspect the Guard config before continuing.

`BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED` (shown as "Cannot auto-edit · fix .env
manually") means the `.env` contains multi-line quoted values, a continuation,
or an unterminated quote. Guard refuses to edit such files at all — no byte is
changed, the error names the line, and consent stays bound so you can retry
after fixing the file. This is a lexical safety refusal, not a network claim
and not a statement that the file is invalid for every reader. Quote a
multi-line value on one physical line, or move Guard's managed block into a
plainly formatted section, then refresh.

If a launch reported a `reused` or `unknown` instance classification, the
activation call completed but an existing Desktop instance may have handled it.
This launch's proxy settings may not have been re-applied. Exit Desktop fully
and launch again from Guard; do not treat the warning as a network failure or
let any tool auto-retry the activation.

## Desktop discovery and activation

Guard must run as a normal, non-elevated user. Elevation-query failure also blocks
launch. Close a running Desktop yourself; Guard never kills it. An uninspectable
matching process is treated as unknown and blocks launch. Registered apps require
an actual supported APPX package and verified FullTrust manifest entry; an EXE
override within a registered installation does not permit bare execution.

`APPX_DISCOVERY_PROTOCOL_INVALID` means the bounded schema/UTF-8/type/path validation
failed. Report the classified error without pasting authentication data. No generic
fallback is attempted. After `APPX_ACTIVATION_OUTCOME_UNKNOWN`, identity failure or
an early exit, check Desktop manually and refresh. Never automatically retry an
activation that may already have been submitted.

## Explicit repair

Normal launch has no daemon effects. Repair requires a fresh single-use
confirmation (CLI `launch --refresh-codex-daemon`, GUI dialog, or console `R` then
`YES`) and can interrupt shared tasks. Registered repair requires backend proxy
authorization; preparation failure guarantees no stop call. Only the public
`codex app-server daemon stop` command is used. Unsupported CLI, malformed output,
non-zero exit, timeout or cancellation blocks activation; shared service outcome
may be unconfirmed. A confirmed stop followed by launch failure is reported as such.

## Package and bridge

Keep the full portable directory. `engine` needs its own Qt6Core.dll and VC runtime,
while the GUI needs Core/Gui/Widgets plus `platforms/qwindows.dll`. Do not mix engine
and GUI from different packages. `build-info` and GUI `--build-info` expose the same
version/commit/dirty provenance. Shutdown must complete with stdin still open.

Bridge failures disable actions and require explicit recovery. stdout is strictly
bounded NDJSON; stderr is not shown verbatim. The GUI cancels and waits for its own
engine only; quitting Guard never terminates Desktop. A submitted activation can
remain unknown even if the helper/bridge has exited. A window whose engine already
failed or crashed still closes normally — no task-manager action is needed. The
proxy editor keeps your input on any save failure; if the engine failed mid-save,
verify the stored settings after reconnecting instead of resubmitting blindly.

Backend proxy block writes require working local NTFS transactions. Microsoft
recommends alternatives to TxF for new applications; this narrow adapter retains
it to enforce the strict concurrent-file preservation contract. If unavailable,
Guard refuses the write/revoke and keeps consent retryable instead of weakening
the transaction. See the architecture decision for details.
