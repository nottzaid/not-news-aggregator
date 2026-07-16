# ADR 0009: Rebuild recoverable GPU surfaces under a finite budget

- Status: accepted
- Date: 2026-07-14

## Decision

The native window and chosen GL configuration outlive a disposable bundle of GL
context, window surface, Skia direct context, and framebuffer wrapper. Explicit
context/surface-loss classifications destroy and rebuild that bundle against
the same window. Two consecutive rebuilds are allowed; one successful
presentation replenishes the budget. A third recoverable failure, allocation
failure, or display loss terminates with the original diagnosis.

If initial GL or a reconstruction cannot supply Skia's interface, the same scene
uses a raster Skia surface presented by Softbuffer. Its RGBA pixels are converted
explicitly to Softbuffer XRGB. Software presentation failure is fatal; cycling
renderers would hide the fault and risk an invisible retry loop.

## Rationale and evidence

Surface loss invalidates pixels, not research. Graph transactions complete
outside presentation, so repaint is sufficient recovery. The application draws
only after invalidation or an animation/work deadline.

Policy tests exhaust and replenish the recovery budget. Native hidden-window
checks cross GL creation, Skia wrapping, presentation, and teardown; forced
checks cross raster creation, conversion, and software presentation. Packaged
self-checks record the actual backend. The optimized 71-event probe measures
input, paint, GPU submission, and swap after warm-up with refresh waiting
removed, while ordinary clocks and presentation remain unchanged.
