# Qt launcher reference and implementation boundary

The supplied GUI brief references Telegram Desktop and Desktop App Toolkit for
compact information density, restrained dialogs, state visibility and component
responsibilities. Their C++ sources, styles, palettes, icons and assets have not
been copied, linked or vendored. The GUI is an original Qt Widgets implementation.

Reference concepts in the brief: `ui/widgets/buttons.*`, `ui/widgets/labels.*`,
`ui/layers/box_content.*`, `ui/toast/*`, `ui/style/*`, `ui/widgets/tooltip.*`,
`ui/widgets/scroll_area.*`, `ui/widgets/rp_window.*`. These are not dependencies.

The product has one native `QMainWindow`, standard buttons and form controls,
small information rows, a controller and a process client. `QProcess` owns only
the C++ bridge. No Qt Network, QML, WebEngine, FFI or Telegram toolkit is used.

Authoritative implementation references:

- [Qt QProcess](https://doc.qt.io/qt-6/qprocess.html): direct program/argument
  execution, asynchronous process signals and bounded application-owned buffers.
- [Qt QStyleHints](https://doc.qt.io/qt-6/qstylehints.html): system color scheme.
- [Qt Windows deployment](https://doc.qt.io/qt-6.8/windows-deployment.html):
  deployment with `windeployqt` and the Windows platform plugin.
- [Qt LGPL obligations](https://www.qt.io/development/open-source-lgpl-obligations):
  runtime replacement, attribution and corresponding source distribution.

See `THIRD_PARTY_NOTICES.md` for the exact shipped Qt version and source hash.
