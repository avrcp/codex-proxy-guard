# Qt GUI acceptance — 0.5.0-rc.1

Execution date: 2026-10-03 (Asia/Shanghai). Platform: Windows 11 x64.
Toolchain: MSVC x64, Qt 6.8.3 dynamic, C++20, Rust workspace lockfile.

## Automated evidence

- Rust format, Clippy with warnings denied, and all 161 workspace tests: PASS
  after configuration transaction and launch lease hardening.
- Qt protocol, fake-client controller and real QProcess/fake-engine suites: PASS.
  They cover strict UTF-8 and framing, unknown states, ID correlation, crash and
  restart, output limits, single foreground work, confirmations, cooperative
  shutdown and both cancel/completion response orders. They do not launch Desktop.
  Both cooperative-shutdown timeout and protocol-failure followed by a hung cleanup
  are exercised against real fake-engine children using the production 20s budget.
- `cargo audit`: exits successfully with the existing `lru 0.18.1`
  `RUSTSEC-2026-0253` allowed unsoundness warning. This release does not remove or
  newly suppress the warning.
- CodeGraph: index current. Diff whitespace check and UI mechanical detector: PASS.
- Native Qt Windows render fixtures: PASS at device pixel ratios 1.0, 1.25, 1.5
  and 2.0, light/dark, 520×430 and 480×400 logical sizes. Screenshots under
  `target/gui-review-final/` are local review evidence, not golden test assertions.
- Tab-focused Refresh + Enter invokes Refresh, not Launch; tested in controller
  tests. Confirmation defaults to Cancel and Home is selectable/read-only.

## Independent review

Protocol/lifecycle/configuration review is performed separately from implementation.
The unavailable dedicated Impeccable reviewer role is substituted by that same
independent read-only reviewer for the screenshots and UI code. The mechanical
detector found no issues; native render review found and corrected a clipped Edit
button. Other identified fixes include native Enter behavior, visible primary
button focus, scrollable error details and obsolete cancellation acknowledgments.
Final independent visual review found no remaining material UI issues. The Windows
configuration handle/launch lease changes were independently checked across all
production call sites. Idle-window close reentrancy was caught by native package
smoke, fixed with queued close delivery, and regression tested.
Qt package provenance additionally verifies the official SDK archive SHA and uses
its complete contents in a fresh isolated build SDK, covering headers, import
libraries and CMake configuration as well as deployed runtime binaries. The
archive-to-runtime manifest verifier was executed successfully before packaging.

## Explicitly not claimed

- Windows 10 execution, clean-machine Windows without any SDK/runtime installation,
  assistive screen-reader exercise and multi-monitor movement: NOT RUN.
- Real Desktop ordinary launch, backend Home modification/revoke, shared-daemon
  repair interruption and live network/business flows: NOT RUN for this GUI change.
  These retain the separate package-identity acceptance matrix. Automated fixtures
  verify backend gates/transactions without touching the user's Home.
- Test-driven DPI rendering is not a claim of complete manual UX acceptance.
- Package loader smoke with an SDK-free PATH is not a clean-machine VM test.
- Unsigned executables are not claimed to have Authenticode verification.

## Final release evidence

Final commands, source provenance and package verification are appended after
the review fixes and canonical packaging complete. Build artifacts remain ignored.
