# Console launcher design

Mode: Operate. The first screen answers which proxy, which application,
whether it is running, and what proxy configuration is prepared. Launch is
the only primary action.

- Fixed full-width console frame, minimum 72x22 (a resize notice below that),
  comfortable at 86x28 and above.
- One aligned block of six information rows, no scrolling or nested cards;
  box-drawing modal dialogs centered over the frame.
- Standard 16-color SGR styling; semantic status text plus color (green
  ready/current/running, yellow pending/stale/not-authorized, red
  conflict/invalid/blocked, dim unknown) — never color alone.
- Alternate screen by policy (`auto`/`always`/`never`), restored on exit
  together with console modes, cursor and window title.
- Line editor with Tab switching, Home/End/Left/Right/Backspace/Delete for
  the proxy host and port; port accepts digits only.
- Consent dialogs show the exact engine-provided Home, require typing YES, and
  default to Cancel (Esc).
- Busy state shows a spinner plus Ctrl+C cancel; destructive keys are gated
  while a foreground operation is active.
- Errors remain visible in a modal until an explicit action supersedes them;
  the copied diagnostic contains only version, error code, public application
  fields and coverage state.

No decorative illustration or third-party icon asset is used; the executable
carries standard VERSIONINFO metadata.
