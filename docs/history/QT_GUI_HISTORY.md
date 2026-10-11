# Qt GUI history (frozen 2026-10-11)

The Qt Widgets frontend, the `bridge` NDJSON engine child and every Qt build
asset were removed by the `feature/win32-tui-no-qt` migration (product
`0.7.0-rc.1`: pure C++20, Win32 console TUI, no Qt, no PowerShell APPX
discovery). This file is the durable record of what was removed, why, and the
last state that shipped it.

## Why removed

The product is a small local proxy launcher; the GUI framework was its largest
binary, build and review cost. The static single-EXE release aggregated
~18.3 MB of static Qt + CRT into one unsigned file whose low-reputation hash
combined with self-relaunching child processes and a PowerShell Appx query made
every new release look novel to antivirus heuristics. Removing Qt and the
bridge self-child shrinks the attack/behavior surface that Guard does not need
while keeping the verified engine semantics byte-for-byte compatible.

Removing Qt does **not** by itself guarantee the absence of antivirus
detections. The accepted claim is only: fewer third-party static bytes, a
smaller EXE, no GUI/bridge self-child, no PowerShell discovery, plus consistent
Authenticode signing and vendor false-positive submissions to build
distribution reputation.

## Final Qt state (rollback point)

- Tag: `qt-gui-v0.6.1-rc.1` → commit `d989a1b9d8706d606ce54641be33a4d74f7043a2`
  (branch `main` at freeze time).
- Last static release built from `6923ebc0024686f424f110d952858097d17c4213`:
  `dist/releases/0.6.1-rc.1-6923ebc0/CodexProxyGuard.exe`, 18,290,688 bytes,
  SHA-256 `aecf9efb4defdf66241cbb5fca16e6942fc729a4d177f9b8da42efd38da84c12`,
  unsigned, import table clean of non-system DLLs. Evidence:
  `docs/CPP_GUI_ACCEPTANCE.md` (2026-10-03 sections).
- Configuration schema: version 2 (`docs/ARCHITECTURE.md` at the tag). The
  no-Qt product keeps reading/writing the same schema and the same
  `%APPDATA%\codex-proxy-guard\config.toml`; rollback restores it untouched.

## What was deleted

`gui/` (Widgets frontend, controller, engine client, licenses viewer),
`backend/src/bridge.*` and the `bridge` process role, `licenses/Qt/` (103
embedded Qt license sections), `resources/appx-discovery.ps1`,
`scripts/build-qt-static.ps1`, `scripts/verify-qt-sdk.ps1`,
`scripts/build-gui.*`, `docs/CPP_GUI_*.md`, `docs/STATIC_QT_REBUILD.md` and the
Qt sections of `THIRD_PARTY_NOTICES.md`.

## Interim regression use

During the migration the Qt GUI remained buildable behind an auto-detected
optional Qt package and drove the migrated engine through the unchanged bridge
protocol as a regression client. It was deleted in the same series (see
`docs/CPP_TUI_MIGRATION_PLAN.md`); the final mainline has no Qt build option,
and rollback is by tag, not by a permanent dual implementation.
