# ADR 10: Native text entry is append-oriented, Unicode-bounded, and optional

Status: accepted

## Decision

`Ctrl+K` or `/` opens a Rust-only research composer without altering Flutter-
matched chrome while closed. The window enables native IME delivery only while
the composer owns text focus. Key text, IME commits, and clipboard text share
one sanitizer: line controls become spaces, other controls disappear, and the
committed question cannot exceed 4,096 Unicode scalars or 16 KiB. Backspace
removes one extended grapheme cluster. IME preedit remains visually distinct
and uncommitted; focus loss clears preedit but retains the question. Enter
submits, Escape closes, `Ctrl+U` clears, and `Ctrl+V` uses the native Windows or
Wayland/X11 clipboard in a confined lifetime.

Editing is deliberately append-oriented. It does not pretend to be a document
editor with arbitrary selection or cursor movement. The visible panel retains
the project's established type, color, border, shadow, and density language;
it clips a bounded tail, names every control, and exposes the committed length.

## Why

Flutter exposed voice as the primary entry and supplied no visible keyboard
composer to copy. A test-only Dart mock would manufacture provenance. Native
text entry is nevertheless necessary when a microphone or transcription key is
unavailable, and IME/paste are desktop input semantics rather than optional
polish. Separate byte and scalar limits bound prompts containing either ASCII
or multi-byte text; grapheme deletion avoids corrupting combining sequences and
joined emoji.

## Evidence

- Unicode tests commit IME text, preserve preedit separation, remove combining
  and joined-emoji graphemes atomically, reject controls, and hit both bounds.
- A raster contract confines the panel and proves long-tail/preedit visibility.
- The platform toggles winit IME ownership from application state; composer
  shortcuts never reach canvas undo/redo handling.
