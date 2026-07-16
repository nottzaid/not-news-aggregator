# ADR 0002: Paint the reference composition directly with Skia

- Status: accepted
- Date: 2026-07-14
- Governing delivery: [issue #2](https://github.com/muradkant/not-news-aggregator/issues/2)

## Problem

The rewrite must preserve Flutter's geometry, typography, clipping, glow,
layering, easing, and timing while removing Flutter itself. The canvas is
spatially unbounded; 1400×900 normalizes reference coordinates, never world
bounds. A stock Rust widget theme cannot mechanically express the original
custom-paint source, and adopting a toolkit only to bypass its layout and paint
model adds an abstraction without gaining its value.

The investigated higher-level options were credible but mismatched: egui favors
tooling widgets over source-level paint parity; Vizia exposes Skia but coarse
full-view invalidation and version coupling remain; Slint offers the strongest
general shell but no equally direct stable paint seam; Vello and Freya were
still moving across gaps that intersect required text, blur, and filters.
winit, glutin, Skia, and Softbuffer are established lower-level components; the
integration risk is ours and is tested as such.

## Decision

```text
winit input → interaction clocks → immutable scene → Skia painter
                                                   ↓
                              OpenGL surface or Skia-raster/Softbuffer
```

`renderer` owns the Flutter-to-Skia translation, bundled Manrope and JetBrains
Mono, cached pictures/text/paths, deterministic offscreen output, and frame
instrumentation. `platform` owns winit and the disposable GL/Skia surface
bundle on both Windows and Linux, with a Skia raster surface presented through
Softbuffer when GL cannot start or is forced off. Only the platform crate may
use narrowly audited unsafe calls required by GL creation and inspection.

Animation and interaction use explicit source-derived clocks and state machines;
the painter cannot mutate graph semantics. Rendering is invalidation-driven and
settled windows sleep until input, work, or an animation deadline. Visual
optimization may cache work or bound scheduling, but may not shorten motion,
change easing, or exceed the decoded-pixel budgets.

## Evidence and consequences

Decoded reference frames cover full and narrow windows, artifact expansion,
neighbor displacement, chrome, activity, and transition endpoints/midpoints.
Native hidden-window checks create, present, resize, and destroy real OpenGL and
forced-raster surfaces; recovery policy bounds repeated recoverable GPU failure.
The 71-event release probe measures the optimized input-to-swap path after
warm-up without changing ordinary presentation policy.

The application consequently owns hit testing, focus, IME, panel layout,
accessibility descriptions, surface recovery, and scheduling. Renderer upgrades
require before/after raster and frame evidence. Adopt a higher-level shell only
if it preserves direct Skia output and raw desktop semantics while removing
more owned code than it adds; replace Skia only when the parity corpus proves a
maintained alternative safer or more exact.
