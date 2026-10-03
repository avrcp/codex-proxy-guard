# Third-party notices

Codex Proxy Guard's own code is MIT licensed; see `LICENSE`. The complete
notice set — this file, `LICENSE`, every Qt license text and the Qt embedded
third-party attributions — is embedded in the release executable and can be
read offline with `CodexProxyGuard.exe licenses`.

## Qt 6.8.3 (static release linkage)

The release executable statically links Qt Core, Gui and Widgets built from
the official QtBase 6.8.3 source release tarball
(`qtbase-everywhere-src-6.8.3.tar.xz`, SHA-256
`56001b905601bb9023d399f3ba780d7fa940f3e4861e496a7c490331f49e0b80`)
configured `-release -static -static-runtime` with MSVC and the static CRT
(/MT). The imported static plugins are the Windows platform integration,
gif/ico/jpeg image codecs and the modern Windows style. Qt Test is used only
for tests and is not linked into the production executable. Qt is copyright
The Qt Company Ltd. and other contributors, used under the GNU Lesser General
Public License version 3 (LGPLv3).

Qt's corresponding source, build scripts, the exact static recipe
(`recipes/qt-static-recipe.json`, keyed by source hash, configure arguments,
architecture and toolchain versions), bundled third-party copyright/license
notices and rebuild instructions are provided as the separate
`CodexProxyGuard-<version>-source-compliance.zip` distributed beside the
runtime executable at the same location (see `SOURCE_ACCESS.txt` and
`docs/STATIC_QT_REBUILD.md`). The upstream source is also available at:

https://download.qt.io/archive/qt/6.8/6.8.3/submodules/qtbase-everywhere-src-6.8.3.tar.xz

LGPLv3 §4(d)(0) compliance for the static combination is met by shipping the
corresponding application source and Qt source together with the toolchain
recipe needed to relink the application against a modified Qt. Rebuilding Qt
from the provided source and relinking the application is explicitly
supported and not blocked by any signature or checksum gate; official
provenance manifests only describe the pinned official recipe. Reverse
engineering for debugging modifications to the LGPL libraries is permitted.

The transitional dynamic profile deploys unmodified Qt shared libraries from
the official MSVC2022 x64 binary SDK archive. The release builder verifies
that archive and every deployed DLL against
`licenses/Qt/sdk-6.8.3-msvc2022-x64.json` (derived from Qt's published
SHA-256), deploying with `--no-patchqt` and a relative `qt.conf` so packaged
Qt DLLs remain byte-identical to it. Run `scripts/verify-qt-sdk.ps1` to
reproduce the archive and 66 member hash checks (requires Python 3 and py7zr).

Qt's third-party notices are additionally copied to `licenses/Qt/third-party/`
in the repository and source-compliance archive, preserving their source
paths. These cover dependencies embedded in QtBase (freetype, harfbuzz,
libpng, pcre2, zlib and friends, statically bundled by the Qt build);
inclusion of a notice does not imply that every optional Qt backend is shipped.

This project does not include Telegram Desktop or Desktop App Toolkit code,
icons, logos, or other assets; those projects are design and architecture
references only, not shipped dependencies.

## Microsoft Visual C++ runtime

The release executable statically links the Visual C++ runtime (/MT); no
`msvcp140*.dll`, `vcruntime140*.dll` or VC redist installer is required or
distributed. The transitional dynamic package ships unmodified x64
redistributable runtime DLLs supplied with the Microsoft Visual C++ build
tools. These components remain subject to Microsoft's license terms; they are
not covered by this project's MIT license. See
https://visualstudio.microsoft.com/license-terms/ and
https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files.

## toml++ 3.4.0

The C++ engine uses the vendored toml++ 3.4.0 single-header TOML parser,
copyright Mark Gillard and contributors, under MIT. Its original license is in
`backend/third_party/toml++/LICENSE` in source and
`licenses/tomlplusplus/LICENSE` in the source-compliance archive. The upstream
URL and pinned source checksum are recorded in that vendored directory's
README. No Rust dependencies are included in this release.
