# Static Qt rebuild guide

This document describes how the pinned static Qt 6.8.3 SDK used by the
single-file release is built, cached, verified and rebuilt — including
rebuilding with local modifications. The authoritative inputs are also shipped
inside every source-compliance archive (`recipes/qt-static-recipe.json`,
`recipes/toolchain.json`, `upstream/qtbase-everywhere-src-6.8.3.tar.xz`).

## Pinned inputs

| Input | Value |
|---|---|
| QtBase source | `qtbase-everywhere-src-6.8.3.tar.xz` |
| Source SHA-256 | `56001b905601bb9023d399f3ba780d7fa940f3e4861e496a7c490331f49e0b80` |
| Upstream URL | https://download.qt.io/archive/qt/6.8/6.8.3/submodules/qtbase-everywhere-src-6.8.3.tar.xz |
| Configure | `-release -static -static-runtime -opensource -confirm-license -nomake examples -nomake tests` |
| Runtime library | static CRT (/MT via `-static-runtime`) |
| Architecture | Windows x64, MSVC 2022 or newer |

No patches are applied to the upstream tarball (`patches/README.txt` records
this in every compliance archive). Qt 6.8's `configure.bat` is a pure CMake
wrapper — no Perl installation is required. The build additionally needs
CMake ≥ 3.22 and Ninja (both bundled with Visual Studio) and Python 3.

## Canonical build

```powershell
.\scripts\build-qt-static.ps1
```

The script verifies the cached source digest, extracts into
`target\qt-static\<recipe-key>\src`, configures in a separate `build`
directory, installs into `install`, and only then writes `manifest.json` plus
a `COMPLETE` marker. The recipe key is a SHA-256 over the source digest,
configure arguments, parallel level, MSVC/SDK/CMake/Ninja versions and the
architecture — a cache can never be silently reused across changed inputs,
and an interrupted build leaves no reusable `COMPLETE` marker.

Post-install verification covers `moc.exe`, `rcc.exe`, `qtpaths.exe`, the
static `Qt6Core/Gui/Widgets/Test` libraries and their CMake package files.
Re-running the script reuses a verified cache and prints its prefix.

## Application build against the static SDK

```powershell
.\scripts\test-cpp.ps1 -StaticQt -QtRoot target\qt-static\<recipe-key>\install -BuildDirectory target\cpp-static-dev
.\scripts\build-portable.cmd -StaticQt
```

`CPG_STATIC_QT=ON` selects `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded` before
`project()` and fails configuration if the configured Qt prefix is not a
static build (imported-target type check). The release pipeline additionally
gates on `dumpbin /DEPENDENTS`: the final EXE must import no Qt, MSVC dynamic
runtime or unexpected third-party DLL.

## Rebuilding with a modified Qt (user freedom)

You may rebuild Qt with your own patches or feature changes and relink the
application:

1. Obtain the same upstream source (from the compliance archive or Qt's
   archive; verify the SHA-256 unless you intend to change it).
2. Configure your own static build, e.g. from a build directory:
   `..\src\configure.bat -prefix C:\my-qt-static -release -static -static-runtime -opensource -confirm-license -nomake examples -nomake tests`
   then `cmake --build . --parallel` and `cmake --install .`.
3. Build the application against your prefix:
   `.\scripts\test-cpp.ps1 -QtRoot C:\my-qt-static -BuildDirectory target\cpp-my-qt`
   (without `-StaticQt` the runtime-library consistency check is skipped, so
   keep your Qt built with the same CRT flavor as your application build).

The canonical release script pins the official recipe for provenance; it does
not prevent local/custom builds, and nothing in the shipped executable
verifies Qt hashes at runtime. A custom build is your own provenance — do not
present it as the official release.

## Maintenance notes

- Updating or fixing Qt means rebuilding the SDK (delete the old key
  directory or let the recipe key change) and releasing a new EXE; users can
  no longer swap DLLs in the static profile.
- The recipe cache lives under `target\qt-static` and survives builds; it is
  not committed to the repository.
- Cross-machine byte-identical outputs are not claimed; only the inputs and
  steps are pinned (see the release checklist).
