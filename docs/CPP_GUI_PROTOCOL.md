# GUI stdio protocol 1

The hidden `codex-proxy-guard bridge` command is a child-process transport, not a
network API. The GUI resolves `engine/codex-proxy-guard.exe` relative to its own
installation, never through PATH or the current directory. stdout is protocol
only; raw stderr is retained in a bounded 64 KiB ring and is never displayed.

Requests are strict UTF-8 NDJSON objects with `schema:1`, positive safe-integer
`id`, `method` and object `params`. Limits exclude the newline: 32 KiB inbound,
128 KiB outbound. Unknown schema/method, malformed JSON, duplicate request fields,
unexpected fields and invalid parameters produce classified errors. Request IDs
are correlated; malformed responses disconnect the GUI without replaying actions.

Methods:

| Method | Parameters / result |
| --- | --- |
| `hello` | `{}` → protocol/engine versions, commit, capabilities |
| `snapshot` | `{}` → safe local state, action flags and confirmation proposals |
| `set_proxy` | `{host,port}` → validate/save through Rust, refreshed snapshot |
| `set_backend_proxy_consent` | `{enabled,confirmation_token}` → existing consent/revoke transaction, snapshot |
| `start_launch` | `{repair,confirmation_token?}` → `{operation_id}`; repair requires token |
| `cancel_operation` | `{operation_id}` → request cooperative cancellation |
| `shutdown` | `{}` → cancel, bounded cleanup, exit |

The snapshot describes proxy, selected application metadata, process state,
coverage, elevation, configuration readiness and permitted actions. It excludes
installation paths, raw configuration, `.env` contents and authentication data.
The exact Home appears only as the necessary consent scope. Unknown GUI states
have a textual fallback and cannot enable launch.

`confirmations.backend_proxy` contains `{token,enabled,home}`;
`confirmations.repair` contains `{token}`. Tokens are single-use session proposals
bound to the server's configuration and consent Home. The GUI displays the Home,
defaults to Cancel and sends only the token/decision, never a replacement Home.
Configuration changes and snapshot refresh invalidate old proposals. Tokens enforce
fresh scope, not authentication against another program controlling the same user.

Only one foreground operation is accepted. While it is active, hello, cancellation
and shutdown remain available; snapshot returns cached busy state without starting
discovery or replacing confirmation proposals. New foreground work returns `BRIDGE_BUSY`.
Launch emits `operation_state` and `operation_finished` with its operation ID.
Cancellation is not success: the real engine result may report an unknown outcome
after submission. The GUI refreshes state after completion and never retries a
launch automatically. On bridge failure it disables actions and offers explicit
Restart Engine.

Closing the GUI requests bridge shutdown and waits for cleanup. Lost/unresponsive
bridges may be terminated only as the GUI's own child, with unknown outcome
reported. Desktop and shared daemon processes are never directly terminated.
