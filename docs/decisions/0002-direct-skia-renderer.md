# ADR 0002: Port Flutter painting through direct Skia surface adapters

- Status: accepted; supersedes ADR 0001's eframe choice and Linux-only scope
- Date: 2026-07-14
- Governing delivery: [issue #2](https://github.com/muradkant/not-news-aggregator/issues/2)

## Context

The rewrite must reproduce Flutter's visible composition, geometry, fonts,
colors, clipping, glow, transition staging, easing, and timing. Its canvas is
unbounded; 1400×900 supplies viewport normalization, never world bounds. The
first eframe frame proved that stock egui painting and widgets do not provide a
mechanical translation of Flutter's retained state and `Canvas` paint code.
Reimplementing every visual through egui would retain the framework while
bypassing the reason to use it.

Windows and Linux are the product platforms. macOS, mobile, and web impose no
design or verification obligations.

The renderer investigation established:

- `skia-safe` 0.99 decoded and registered both bundled fonts; SkParagraph,
  quadratic paths, path measurement, blur masks, and radial-gradient shaders
  constructed and executed.
- rust-skia supplies current GL, Vulkan, and Direct3D window examples and
  prebuilt Windows/Linux bindings. A probe that unnecessarily enabled embedded
  FreeType missed the cache and compiled roughly 1,450 C++ objects; Vizia's
  cache-supported feature set built cleanly in about 34 seconds.
- winit/glutin are maintained low-level infrastructure used by Alacritty.
  Slint's current renderer uses `skia-safe` with glutin and selects Direct3D on
  Windows and GL/Vulkan on Linux. The substrate is established; our integration
  remains our responsibility.
- Released Vizia 0.4 exposed an unmediated Skia canvas, accepted physical pointer
  coordinates, supplied capture, and presented the probe correctly at 1.5×
  scale. A full-window custom view, however, makes its public damage tracker
  invalidate the full canvas, bypasses most widget/layout value, and binds the
  app to Vizia's released Skia 0.93 while its unreleased branch has moved to
  0.99.
- Vello declares itself alpha and lists blur/filter and glyph-cache work; Freya
  warns that its active branch follows a near-total rewrite. Neither belongs in
  this compatibility-critical path.

## Decision

Paint the application directly with `skia-safe`; do not adopt a general Rust GUI
framework.

```text
winit events ──→ interaction/state ──→ immutable scene ──→ Skia painter
                                                        │
                       Linux GL surface adapter ─────────┤
                       Windows surface adapter ──────────┘
```

- `app` owns platform-neutral interaction, animation clocks, hit testing,
  accessibility descriptions, scene construction, and composition.
- `renderer` owns Flutter-to-Skia paint translation, font assets, path/effect
  caches, damage calculation, frame instrumentation, and deterministic
  offscreen rendering.
- A narrow platform crate owns the winit lifecycle and GPU surface/context
  creation. Linux starts with the official glutin GL path. Windows uses a
  separately verified Skia backend adapter; Direct3D is preferred, with GL only
  if measured compatibility justifies it.
- Platform adapters may contain audited unsafe calls required by window/GPU
  APIs. No other crate may weaken the workspace's unsafe-code prohibition.
- Animation and interaction use explicit clocks/state machines derived from the
  Flutter source, not framework defaults. The painter consumes immutable frame
  state and cannot mutate graph semantics.
- Cache static Skia pictures/text/path geometry and compute scene damage. A
  pointer move invalidates only the old/new dynamic bounds unless an active
  whole-scene effect demonstrably requires more.
- Retain settled collision layouts and draw flowing dashes through Skia's
  native path effect. Motion clocks, layer order, and Flutter raster budgets
  remain optimization invariants; a cache that changes a presented frame is
  invalid regardless of measured gain.
- Bundle Manrope and JetBrains Mono. Never rely on host font discovery for
  compatibility rendering.
- Use cache-supported rust-skia features in normal builds. CI verifies both a
  cache hit path and a documented source-build fallback; release artifacts do
  not require end users to install Clang or compile Skia.

## Alternatives

| Choice | Result |
|---|---|
| eframe/egui | Rejected: mature tooling UI, wrong paint/widget semantics; the diagnostic frame failed fidelity. |
| Vizia over Skia | Rejected: capable direct canvas, but our full custom surface forfeits its main value and inherits coarse damage plus release lag. |
| Slint with Skia | Rejected: strongest higher-level toolkit, but no equally direct stable paint seam for a mechanical Flutter painter port. |
| Direct winit/wgpu/Vello | Rejected now: Vello's own alpha gaps intersect required blur, filters, and text. |
| Flutter UI with Rust core | Rejected as target: immediate visual fidelity, but preserves the two-toolchain frontend the rewrite is intended to retire. |

## Consequences

- Exact appearance becomes a source translation and differential-rendering
  problem, not a theme approximation problem.
- We own hit testing, focus, text entry, accessibility integration, panel
  layout, and animation scheduling. Their scope is bounded by the reference
  application and must be covered by executable interaction checks.
- Linux and Windows may differ below the Skia canvas. Synchronized offscreen and
  on-window captures must distinguish painter divergence from backend/driver
  divergence.
- The platform crate is the only audited unsafe boundary and receives lifecycle,
  resize, scale-factor, device-loss, teardown-order, and fallback tests.
- Renderer upgrades are deliberate compatibility changes: pin them, capture
  before/after images and frame traces, and record accepted raster deltas.

## Reversal conditions

Adopt a higher-level shell only if it exposes direct Skia painting, bounded
damage, raw/captured pointer and IME events, accessibility, and Windows/Linux
backends while reducing owned code without altering canonical output or timing.
Replace Skia only if the source-derived parity corpus proves it cannot meet the
visual contract or a maintained renderer matches it with materially lower build
and operational risk. Change a platform backend without revisiting the painter
when on-window evidence shows driver or packaging failure.
