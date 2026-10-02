# Desktop launcher design

The user-supplied implementation brief pins the visual direction. Mode: Operate.
The first screen answers which proxy, which application, whether it is running,
and what proxy configuration is prepared. Launch is the only primary action.

- Native resizable Windows frame, initial 520×430 logical pixels, minimum 480×400.
- Segoe UI Variable with Segoe UI fallback, native system scale.
- One aligned group of six information rows, no navigation or nested cards.
- Original blue accent, neutral surfaces, semantic status text and colors.
- Follow `QStyleHints::colorSchemeChanged` at runtime.
- Standard buttons, dialogs, text fields and spin box preserve keyboard semantics.
- Consent dialogs show the exact Rust-provided Home and default to Cancel.
- Busy state offers Cancel; lost engine offers explicit Restart Engine.
- Errors remain visible until an explicit operation supersedes them; diagnostics
  copy only version, error code, application version and coverage state.

No decorative illustration or third-party icon asset is needed for this utility.
