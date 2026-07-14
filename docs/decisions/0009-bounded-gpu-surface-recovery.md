# ADR 9: Rebuild recoverable GPU surfaces without concealing fatal driver failure

Status: accepted

## Decision

The native window and selected OpenGL configuration outlive a disposable bundle
containing the current GL context, window surface, Skia direct context, and Skia
framebuffer wrapper. `ContextLost`, `BadContext`, `BadContextState`,
`BadCurrentSurface`, and `BadSurface` tear down that bundle, rebuild it against
the existing window, and request a fresh frame. Two consecutive rebuilds are
allowed; a successful presentation replenishes the budget. The third
consecutive recoverable failure and non-surface failures such as allocation or
display loss terminate with the original error.

If GPU initialization or a recoverable reconstruction cannot produce the GL
interface Skia requires, the same Skia scene uses a raster surface presented by
Softbuffer. The software path converts explicit RGBA pixels to Softbuffer's
XRGB contract and remains invalidation-driven; it is compatibility insurance for
virtual machines, old drivers, and remote desktops, not the preferred backend.
Software presentation failure remains fatal rather than cycling renderers.

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
  surface creation, Skia wrapping, presentation, and clean exit; a forced mode
  crosses raster creation, pixel conversion, software presentation, and exit.
- Linux CI runs both backends; Windows CI runs auto-selection natively, so a
  driverless runner proves the fallback instead of receiving a compile-only
  exemption.
