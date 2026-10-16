# Rust Migration Baseline (frozen contracts)

Frozen on branch `refactor/rust-ratatui-single-exe` from `main@0bdf2ed586a9f12f99ff411bd437841fcd6f5433`
(C++ product version `0.7.0-rc.4`). The Rust rewrite targets `0.8.0-rc.1` and must
preserve every contract recorded here. Goldens live in `tests/golden/` and were
captured from the C++ executable built from this exact HEAD
(`361b613ace34b9413cdf41c4b3504718da286a0915756b66437519245e129c2d`,
build provenance `commit 8c67b85b6154c3a6ad3068533ae53277d0fbc524, dirty=false`).

## Golden fixture provenance

All fixtures were produced by running the C++ binary with only no-side-effect
commands: `--help`, `--version`, `build-info`, `config-path`, `licenses`,
`init-config` (temp paths), invalid-CLI probes, and `console --plain` proxy-edit
rewrites on temp configurations. No command launched, activated or stopped any
real Desktop/daemon state. `launch --json` was invoked exactly once against a
fresh temp config and was correctly refused by the running-process guard; that
environment-dependent refusal is recorded as a behavioral fact, not a portable
byte golden.

## Frozen public contract facts

1. **CLI** (`tests/golden/help.txt`, error probes): exact usage text; exit code 1
   with `CODE: message` on stderr for human mode and `{"error":{"code":…}}` on
   stdout for `--json`; duplicate value options, unknown options/commands,
   `--version --json` combination, empty option values all rejected with
   `CLI_INVALID`; `-h/-?/--help`, `-v/--version`; `--config` accepts CJK and
   relative paths (resolved to absolute, lexically normalized).
2. **Version/build-info**: `codex-proxy-guard <version>` line; build-info JSON
   keys `commit`, `dirty`, `language` (`C++20` → `Rust` is the single approved
   change), `version`, alphabetical key order.
3. **config.toml serialization** (`default-config.toml`, roundtrip fixtures):
   toml++ canonical form — top-level scalars first, then tables alphabetically
   (`[codex]`, `[proxy]`, `[tui]`) with a blank line before each, keys
   alphabetical inside tables, integers bare, strings literal-quoted `'…'`
   unless the value contains `'` (then basic `"…"` with `\\` escapes for
   backslashes, apostrophe raw), arrays as `[ 'a', 'b' ]`, exactly one trailing
   newline. Parsing accepts any valid TOML v2 schema instance; unknown keys,
   wrong types, non-UTF-8, >64 KiB are `CONFIG_INVALID`.
4. **Redaction** (`publicText`): USERPROFILE (both slash forms, case-insensitive)
   → `%USERPROFILE%`, URI credentials → `[REDACTED]`, secret-prefix lines
   (`authorization:`, `cookie:`, `token=`, `api key=`, … earliest per line) →
   `[REDACTED]`, ANSI CSI/OSC removal, control/format/whitespace filtering,
   whitespace collapsing, code-point-safe truncation.
5. **Strict JSON**: ≤ size limit, strict UTF-8, optional BOM skip, depth ≤ 64,
   duplicate keys rejected at every level including escape-decoded spellings,
   exactly one top-level object, no trailing data.
6. **`.env` managed block**: markers `# BEGIN/END CODEX PROXY GUARD: proxy-v1`,
   exactly `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` inside, outside bytes (incl.
   CRLF) preserved verbatim; multi-line quote/continuation →
   `BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED` with the 1-based first refused line;
   conflicts (case-folded, `export KEY=` included) → `BACKEND_PROXY_CONFIG_CONFLICT`;
   unknown marker versions and malformed blocks → `BACKEND_PROXY_BLOCK_INVALID`;
   all writes through TxF (`CreateFileTransactedW` + expected-bytes re-read in
   transaction), unavailable TxF → `BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE`, no
   fallback.
7. **Session invariants**: single foreground gate; revision-bound single-use
   consent/repair tokens consumed on any submission attempt; timeout →
   `CancellingAfterTimeout` until the future is reaped; receipts never
   overwritten by later refresh errors; mutation `committed` fact preserved even
   when the following refresh fails.
8. **Launch pipeline order**: capability checks → discovery → process-stopped
   gate → metadata gate → (registered only) backend block prepare before any
   daemon stop → exactly one `codex app-server daemon stop` on repair →
   re-discovery identity re-check → startup-lock mark → activation
   (AUMID/AO_NONE via same-EXE worker, 16 KiB request / 64 KiB receipt / 30 s)
   → receipt with `activation_state`, `instance`, `proxy_delivery`,
   `backend_proxy_config`, `daemon_preparation`.
9. **Error-code vocabulary** (source-scan, stable public surface):
   `APPX_ACTIVATION_COM_INIT_FAILED`, `APPX_ACTIVATION_FAILED`,
   `APPX_ACTIVATION_MANAGER_UNAVAILABLE`, `APPX_ACTIVATION_OUTCOME_UNKNOWN`,
   `APPX_ACTIVATION_PROTOCOL_INVALID`, `APPX_ACTIVATION_WORKER_FAILED`,
   `APPX_APPLICATION_AMBIGUOUS`, `APPX_APPLICATION_MISSING`,
   `APPX_DISCOVERY_COM_FAILED`, `APPX_DISCOVERY_FAILED`,
   `APPX_DISCOVERY_LIMIT`, `APPX_DISCOVERY_PROTOCOL_INVALID`,
   `APPX_DISCOVERY_PROTOCOL_UNSUPPORTED`, `APPX_DISCOVERY_TIMEOUT`,
   `APPX_DISCOVERY_UNSTABLE`, `APPX_EXECUTABLE_INVALID`, `APPX_IDENTITY_MISMATCH`,
   `APPX_IDENTITY_MISSING`, `APPX_IDENTITY_QUERY_FAILED`, `APPX_AUMID_MISMATCH`,
   `APPX_AUMID_MISSING`, `APPX_AUMID_QUERY_FAILED`, `APPX_INSTALL_LOCATION_INVALID`,
   `APPX_MANIFEST_ATTRIBUTE_UNSUPPORTED`, `APPX_MANIFEST_READ_FAILED`,
   `APPX_METADATA_INCOMPLETE`, `APPX_NOT_FOUND`, `APPX_PACKAGE_CHANGED`,
   `APPX_OVERRIDE_INVALID`, `APPX_TARGET_ELEVATED`,
   `APPX_TARGET_ELEVATION_QUERY_FAILED`, `APPX_TARGET_EXITED_EARLY`,
   `APPX_TARGET_VERIFY_FAILED`, `ACTIVATION_ONLY_UNSUPPORTED`,
   `BACKEND_PROXY_BLOCK_INVALID`, `BACKEND_PROXY_CONFIG_CONFLICT`,
   `BACKEND_PROXY_CONSENT_SAVE_FAILED`, `BACKEND_PROXY_ENV_ATOMIC_UNAVAILABLE`,
   `BACKEND_PROXY_ENV_ENCODING`, `BACKEND_PROXY_ENV_FAILURE`, `BACKEND_PROXY_ENV_IO`,
   `BACKEND_PROXY_ENV_SYNTAX_UNSUPPORTED`, `BACKEND_PROXY_REVOKE_FAILED`,
   `BACKEND_PROXY_REVOKE_REQUIRED`, `BACKEND_PROXY_REQUIRED_FOR_REPAIR`,
   `BACKEND_PROXY_SCOPE_UNCONFIRMED`, `CLI_INVALID`, `CODEX_ALREADY_RUNNING`,
   `CODEX_DAEMON_STOP_FAILED`, `CODEX_DAEMON_STOP_TIMEOUT`, `CODEX_DAEMON_UNSUPPORTED`,
   `CODEX_EXECUTABLE_INVALID`, `CODEX_EXECUTABLE_MISSING`, `CODEX_HOME_INVALID`,
   `CODEX_HOME_UNRESOLVED`, `CODEX_LAUNCH_FAILED`, `CODEX_NOT_INSTALLED`,
   `CODEX_RUNNING_UNKNOWN`, `CONFIG_CHANGED`, `CONFIG_CONSENT_REPAIR_REQUIRED`,
   `CONFIG_INVALID`, `CONFIG_LOCK_FAILED`, `CONFIG_PATH_INVALID`,
   `CONFIG_PATH_UNAVAILABLE`, `CONFIG_READ_FAILED`, `CONFIG_ROLLBACK_FAILED`,
   `CONFIG_TOO_LARGE`, `CONFIG_WRITE_FAILED`, `CONFIRMATION_REQUIRED`,
   `CONSOLE_INPUT_FAILED`, `CONSOLE_OUTPUT_FAILED`, `ENGINE_TASK_FAILED`,
   `ENVIRONMENT_INVALID`, `ENVIRONMENT_UNAVAILABLE`, `ELEVATION_NOT_ALLOWED`,
   `EXECUTABLE_PATH_UNAVAILABLE`, `GUARD_ELEVATED`, `GUARD_ELEVATION_QUERY_FAILED`,
   `HELPER_BUDGET_INVALID`, `HELPER_CLEANUP_FAILED`, `HELPER_DRAIN_TIMEOUT`,
   `HELPER_IO_FAILED`, `HELPER_OUTPUT_LIMIT`, `HELPER_START_FAILED`,
   `HELPER_TIMEOUT`, `INPUT_INVALID`, `INVALID_LAUNCH_OPTIONS`,
   `LAUNCH_BUSY`, `LAUNCH_CANCELLED`, `PROXY_BYPASS_UNSUPPORTED`,
   `PROXY_LAUNCH_PLAN_INVALID`, `SESSION_BUSY`, `SESSION_PARAMS_INVALID`,
   `SESSION_RANDOM_UNAVAILABLE`, `SESSION_TIMEOUT`, `TEXT_ENCODING_INVALID`.
10. **Environment facts on the reference machine** (context only, not a
    portable golden): a real ChatGPT Desktop package is installed and was
    running during capture; native AppModel discovery is exercised read-only by
    the engine tests. Automated Rust tests must keep using temp Homes and fake
    services and must never activate the real Desktop.

## Not frozen here

Real-machine acceptance evidence (A01–A14 in the implementation manual) is
`NOT_RUN` for the Rust build until a human operator executes it; the C++
history in `docs/RELEASE_READINESS.md` carries over only as history, never as
inherited PASS.
