# Third-party notices

Codex Proxy Guard's own code is MIT licensed; see `LICENSE`. The complete
notice set — this file, `LICENSE`, and the vendored third-party licenses
below — is embedded in the release executable and can be read offline with
`CodexProxyGuard.exe licenses`. The 0.6.x Qt-based releases are historical
(see `docs/history/QT_GUI_HISTORY.md`); this release links no Qt code.

## nlohmann/json 3.12.0

The engine uses the vendored nlohmann/json 3.12.0 single-header JSON library
for every protocol parse/serialize boundary, copyright Niels Lohmann and
contributors, under MIT. The original license is in
`backend/third_party/json/LICENSE.MIT`; the upstream URL and the pinned
SHA-256 of the vendored header are recorded in that directory's README. No
configure-time download is used anywhere in the build.

## toml++ 3.4.0

The engine uses the vendored toml++ 3.4.0 single-header TOML parser,
copyright Mark Gillard and contributors, under MIT. Its original license is
in `backend/third_party/toml++/LICENSE`; the upstream URL and pinned source
checksum are recorded in that vendored directory's README.

## doctest 2.4.12 (test binaries only)

The doctest suites use the vendored doctest 2.4.12 single-header framework,
copyright Viktor Kirilov, under MIT. Its original license is in
`tests/third_party/doctest/LICENSE.txt`. It is never linked into the
production executable or any shipped artifact.

## Microsoft Visual C++ runtime

The release executable statically links the Visual C++ runtime (/MT); no
`msvcp140*.dll`, `vcruntime140*.dll` or VC redist installer is required or
distributed. These components remain subject to Microsoft's license terms;
they are not covered by this project's MIT license. See
https://visualstudio.microsoft.com/license-terms/ and
https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files.

This project does not include Telegram Desktop or Desktop App Toolkit code,
icons, logos, or other assets; those projects are design and architecture
references only, not shipped dependencies.
