# RC3 interaction and launch remediation

This work completes the no Qt interaction and launch repair on
`feature/win32-tui-no-qt`, starting at
`e342456f1dc8e07e6448edeaf68bf985abcfc855`. The initial remote main was
`d989a1b9d8706d606ce54641be33a4d74f7043a2` (feature ahead 19, behind 0).
The intended integration is a tested fast forward to main, followed by an
unsigned development package from that exact clean commit. No public release
or tag is part of this work.

## Development plan

1. Unify Unicode arguments, strict option/port validation and absolute config
   resolution. Information commands and the activation worker avoid config.
2. Enforce the actual child handle list. Settle writer and both readers before
   the verdict; bound drain, reap the direct child, and retain submission facts.
3. Expose typed session views and immutable single use proposals. Separate
   mutation commits, refresh errors, launch receipts and budget observations.
4. Exercise the production TUI controller. Preserve failed drafts, apply busy
   immediately, retain launch results, refresh once, and settle before exit.
   Restore acquired terminal state before plain fallback; clip every modal.
5. Bound package enumeration and fail closed on missing FullTrust evidence,
   image queries and elevation. Keep reused/unknown observations conservative.
6. Capture release inputs with git archive, isolate intermediate directories,
   use an exclusive release lock and temporary smoke config. Run tests, review,
   update documentation, commit and push feature, integrate main, then package.

All automated effects use disposable configurations/Homes and fake Desktop or
owned test children. They do not launch the installed Desktop, stop a shared
daemon or grant/revoke a real Home. Existing consent and file safety rules are
retained; internal GUI/bridge compatibility is removed.

## Regression baseline

Executed against the old implementation before repair on 2026-10-13:

| Area | Observed failure |
|---|---|
| Entry | Empty config entered plain mode; invalid ports and duplicates did not consistently report CLI_INVALID |
| Handle inheritance | Inheritable sentinel was signalled by the child; child exited 7 instead of 0 |
| TUI | Host Enter, draft retention, nullable entry rendering and result R/Help key handling failed |
| Session | Idle cancellation invalidated an available consent proposal |
| Platform | Optional null secondary application and null/elevated activation receipt regressions failed |

Local baseline evidence is in the ignored `target/rc3-entry-baseline`,
`target/process-r03`, `target/cpp-tui-rc3` and `target/cpp-session-rc3` test logs.
These failures are baseline evidence, not final acceptance results.

## Acceptance scope

The final per requirement test matrix is recorded in [TUI acceptance](TUI_ACCEPTANCE.md).
The release manifest, embedded build info and SHA256SUMS are the authoritative
commit and binary records. A source document cannot include its own final
commit hash without changing that hash.

Explorer double click, Windows Terminal, live Desktop activation, real Home
authorization/revocation, shared daemon repair and AV/SmartScreen distribution
remain separate human attended acceptance items unless explicitly recorded as
executed. Their absence does not imply PASS or a public distribution guarantee.

## Official contracts checked

- [Console handler lifetime and shutdown deadlines](https://learn.microsoft.com/en-us/windows/console/handlerroutine)
- [Console control registration and signal delivery](https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler)
- [Synchronous I/O cancellation requires completion settlement](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelsynchronousio)
- [Manifest string attributes and absent values](https://learn.microsoft.com/en-us/windows/win32/api/appxpackaging/nf-appxpackaging-iappxmanifestapplication-getstringvalue)
