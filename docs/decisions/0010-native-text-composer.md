# ADR 0010: Keep native text entry append-oriented and Unicode-bounded

- Status: accepted
- Date: 2026-07-14

## Decision

`Ctrl+K` or `/` opens a Rust-native research composer without changing closed
canvas chrome. IME is enabled only while it owns focus. Key text, IME commits,
and clipboard input share one sanitizer: line controls become spaces, other
controls disappear, and committed input stops at 4,096 Unicode scalars or
16 KiB. Backspace removes one extended grapheme. Preedit remains visibly
distinct and uncommitted; focus loss clears preedit but retains the question.
Enter submits, Escape closes, `Ctrl+U` clears, and `Ctrl+V` uses a confined
native clipboard lifetime.

The editor is intentionally append-oriented: it does not imply arbitrary cursor
or selection semantics. Its panel uses the established type, color, border,
shadow, and density language, clips a bounded tail, names controls, and displays
committed length.

## Rationale and evidence

The reference application privileged voice but lacked discoverable keyboard
input. Text remains essential when microphone or transcription is unavailable;
IME and paste are desktop input contracts, not ornament. Scalar and byte bounds
cover both ASCII and multibyte prompts; grapheme deletion preserves combining
sequences and joined emoji.

Tests exercise IME/preedit separation, grapheme deletion, control sanitization,
both bounds, panel clipping, and focus routing that prevents composer shortcuts
from reaching canvas undo/redo.
