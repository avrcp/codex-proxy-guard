# Third-party notices

Codex Proxy Guard's own code is MIT licensed; see `LICENSE`.

## Qt 6.8.3

The GUI dynamically links the unmodified Qt Core, Gui and Widgets libraries from
QtBase, and deploys the QtBase Windows platform and supporting plugins.
Qt Test is used only for tests and is not a runtime dependency. Qt is copyright
The Qt Company Ltd. and other contributors. The distributed Qt libraries are used
under the GNU Lesser General Public License version 3 (LGPLv3).

The full LGPLv3 and GPLv3 texts are included in `licenses/Qt/`. Qt's corresponding
source, build scripts and bundled third-party copyright/license notices are
included in the portable ZIP as `sources/qtbase-everywhere-src-6.8.3.tar.xz`.
Its SHA-256 is recorded in `build-info.json`. The upstream source is available at:

https://download.qt.io/archive/qt/6.8/6.8.3/submodules/qtbase-everywhere-src-6.8.3.tar.xz

The release builder verifies Qt SDK libraries, plugins and build/deployment tools
against `licenses/Qt/sdk-6.8.3-msvc2022-x64.json`. This manifest was derived from
Qt's official MSVC2022 x64 archive after verifying its published SHA-256, and
records the binary archive URL and matching source archive. Deployment uses
`--no-patchqt` and a relative `qt.conf`, so packaged Qt DLLs remain byte-identical
to that archive. `build-info.json` records both SDK and deployed hashes. These
checks run only while building a release; they do not restrict runtime DLL replacement.

Run `scripts/verify-qt-sdk.ps1` to reproduce the archive SHA-256 and all 66
archive-member hash checks. The GUI builder runs this verification automatically,
reusing the archive in `target/qt-source/`. For each build it extracts the entire
verified archive into a new isolated `target/qt-sdk-*` directory and uses that
SDK for compilation and deployment. Headers, libraries and CMake configuration
therefore also come from the pinned archive; external `QTDIR`/SDK installations
are not build inputs. Verification requires Python 3 and
`py7zr` (installed by `aqtinstall`, or `python -m pip install --user py7zr`).

Qt's third-party notices are also copied to `licenses/Qt/third-party/`, preserving
their source paths. These cover dependencies embedded in QtBase; inclusion of a
notice does not imply that every optional Qt backend is shipped.

You may replace the Qt DLLs and plugins with your own ABI-compatible modified
versions. No signature or checksum enforcement prevents replacement. Reverse
engineering for debugging modifications to the LGPL libraries is permitted.
Build the replacement QtBase with MSVC x64 and CMake using the instructions in
the included source (`README.md`, `configure.bat`, and `CMakeLists.txt`); Qt's
Windows build instructions are at https://doc.qt.io/qt-6.8/windows-building.html.
Keep its public ABI and release runtime compatible with this application.

Project source and GUI build instructions are available at
https://github.com/avrcp/codex-proxy-guard and `docs/CPP_GUI_REFERENCE.md`.
The GUI is not statically linked with Qt. It does not include Telegram Desktop
or Desktop App Toolkit code, icons, logos, or other assets; those projects are
design and architecture references only, not shipped dependencies.

## Microsoft Visual C++ runtime

The package includes unmodified x64 redistributable runtime DLLs supplied with
the Microsoft Visual C++ build tools. These components remain subject to
Microsoft's license terms; they are not covered by this project's MIT license.
See https://visualstudio.microsoft.com/license-terms/ and
https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files.

## toml++ 3.4.0

The C++ engine dynamically links Qt Core and uses the vendored toml++ 3.4.0
single-header TOML parser, copyright Mark Gillard and contributors, under MIT.
Its original license is in `backend/third_party/toml++/LICENSE` in source and
`licenses/tomlplusplus/LICENSE` in the portable package. The upstream URL and
pinned source checksum are recorded in that vendored directory's README.
No Rust dependencies are included in this release.
