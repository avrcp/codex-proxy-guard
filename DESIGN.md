# Console launcher design

Mode: Operate. The first screen answers which proxy, which application,
whether it is running, and what proxy configuration is prepared. Launch is
the only primary action.

- Fixed full-width console frame, minimum 72x22 (a resize notice below that),
  comfortable at 86x28 and above.
- One aligned block of seven information rows, no scrolling or nested cards;
  each label keeps its own cell, the facts wrap instead of being cut off, and
  a terminal under 72 columns stacks the two columns into one. Box-drawing
  modal dialogs are centered over the frame and grow with their content, so
  the last fact of a result is on screen rather than behind the hint row.
- Standard 16-color SGR styling; semantic status text plus color (green
  ready/current/running, yellow pending/stale/not-authorized, red
  conflict/invalid/blocked, dim unknown) — never color alone.
- Alternate screen by policy (`auto`/`always`/`never`), restored on exit
  together with console modes, cursor and window title.
- Line editor with Tab switching, Home/End/Left/Right/Backspace/Delete for
  the proxy host and port; port accepts digits only.
- Ignore key-release events before dispatch or UI state changes. Key presses
  and repeats remain active for normal typing and held-key editing.
- Consent dialogs show the exact engine-provided Home, require typing YES, and
  default to Cancel (Esc).
- Busy state shows a spinner plus Ctrl+C cancel; destructive keys are gated
  while a foreground operation is active.
- Errors remain visible in a modal until an explicit action supersedes them;
  the copied diagnostic is one field per line — version, commit, configuration
  path, error codes, the configured endpoint with its source, the launch-plan
  state, public application fields, coverage state and the retained receipt
  facts — each line redacted on its own, so a long path cannot swallow the
  fields after it.

## Stated facts, never inferred ones

- The PROXY row shows the endpoint the engine last read from the configuration
  file; a failed read keeps that endpoint and marks it `last good read` instead
  of falling back to the built-in default, which would read as "my port was
  reset". A just-committed value is `saved, not re-read`, bytes Guard read but
  could not parse are `stored file not usable`, and a session that has read
  nothing shows `-` labelled `not read yet`: there is no endpoint to display,
  and the built-in 10808 is not a fact about the user's file. The engine says
  which of those states carries an endpoint (`endpoint_known`), so no frontend
  prefills or reuses a port it never published; the proxy editor opens blank.
- Coverage is only claimed for a configuration that was actually read: after a
  failed read the row keeps the endpoint for display and states no backend
  authorization, because the stored consent is unknown at that moment.
- The PLAN row repeats what the engine says about delivery: activation
  arguments, process environment, `not confirmed` before a successful read,
  or `not buildable` with the refusal code. The frontends never recompute it.
- A launch result states what was submitted (`Configured`/`Submitted`
  endpoints, method, delivery channel, PID, instance) and that Guard verifies
  those delivery facts only — Desktop adoption and network use stay unverified.
  The comparison between the two endpoints is an engine fact, so the TUI, the
  line console and the headless CLI describe one receipt in one way. A failed
  launch states that no delivery receipt was recorded, which is also true for
  an activation whose outcome is unknown.
- The blocked reason names the state that is actually true: an unusable stored
  configuration is offered as a repair (saving the proxy is the fix), a failed
  read asks for a refresh, and a session that has read nothing yet says so.

No decorative illustration or third-party icon asset is used; the executable
carries standard VERSIONINFO metadata.
