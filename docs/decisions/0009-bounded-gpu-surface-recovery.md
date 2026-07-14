# ADR 9: Rebuild recoverable GPU surfaces without concealing fatal driver failure

Status: accepted

## Decision

The native window and selected OpenGL configuration outlive a disposable bundle
containing the current GL context, window surface, Skia direct context, and Skia
framebuffer wrapper. `ContextLost`, `BadContext`, `BadContextState`,
`BadCurrentSurface`, and `BadSurface` tear down that bundle, rebuild it against
the existing window, and request a fresh frame. Two consecutive rebuilds are
allowed; a successful presentation replenishes the budget. The third
consecutive recoverable failure, any reconstruction failure, and non-surface
failures such as allocation or display loss terminate with the original error.

## Why

Surface invalidation is recoverable application infrastructure, not corrupted
research state. Terminating on glutin's explicit recoverable classifications
turns a driver reset or window-surface replacement into needless data-access
loss. Retrying without a bound can instead create an invisible CPU loop and
erase the diagnostic that matters.

The application renders only on invalidation or a declared animation deadline.
A failed presentation may therefore discard pixels but cannot partially commit
graph state; durable mutations happen before and independently of the GPU
boundary. Repainting after reconstruction is the correct recovery operation.

## Evidence

- A policy test exhausts consecutive recoverable failures, replenishes the
  budget after success, and rejects allocation/display failures.
- The hidden-window executable crosses native window creation, GL context and
  surface creation, Skia wrapping, presentation, and clean exit.
- Linux and Windows CI run that executable natively; compilation alone is not
  accepted as surface evidence.
