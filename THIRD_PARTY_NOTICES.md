# Third-party notices

Codex Proxy Guard's own code is MIT licensed; see `LICENSE`. The complete
notice set — `LICENSE` and the third-party licenses below — is embedded
in the release executable and can be read offline with
`CodexProxyGuard.exe licenses` (`app/src/licenses.rs` +
`third-party-licenses/`). Historical C++/Qt dependencies (toml++,
nlohmann/json, doctest, Qt) are no longer linked; see git history for the
retired implementations.

## ratatui (MIT)

The terminal interface is built on ratatui (widgets, layout, buffer
rendering). License: `third-party-licenses/ratatui.txt`.

## crossterm (MIT)

Terminal raw mode, events and screen management come from crossterm.
License: `third-party-licenses/crossterm.txt`.

## toml (MIT OR Apache-2.0)

Configuration parsing uses the toml crate. Both upstream licenses are
embedded: `third-party-licenses/toml.txt`.

## windows-rs (MIT OR Apache-2.0)

All Win32 and COM bindings (handles, processes, TxF, console, AppModel,
activation manager, shell known folders) come from the `windows` crate.
Both upstream licenses are embedded:
`third-party-licenses/windows-rs.txt`.
