# Flutter raster oracles

These images are compatibility evidence, not hand-authored design assets.

`background-320x180.png` records `CanvasBackgroundPainter` at DPR 1 through
`test/background_oracle_test.dart`:

```text
Flutter framework 3.44.1, revision 924134a44c (2026-05-29)
Flutter engine c416acfeb8 / hash 39b1f7043775b9578bbb26a1676e79c4e31c8b5e
Reference application commit 3220d3af5607d27b8d945026f8c0551921a4addc
PNG sha256 edd55d1182bbff0334f01b33297118fd92dcf244e35fe7ab6ad9500d79df6134
```

Regenerate only after reviewing the reference-source change and the resulting
Flutter/Rust pixel delta:

```sh
flutter test test/background_oracle_test.dart --update-goldens
```

The Rust check decodes pixels before comparison; PNG encoder bytes are outside
the contract. Its tight nonzero allowance isolates Skia/engine raster-version
variance and must not be widened to absorb changed geometry or color.

`closed-graph-1400x900.png` and `closed-graph-480x270.png` record the real
`_EventCanvasPainter` at DPR 1 with two fixed events, one bridge, zero animation
progress, and application fonts loaded explicitly—the default Flutter-test
`Ahem` font is not design evidence. The first frame tests native reference
geometry; the second forces subpixel scaling:

```text
Flutter and reference source revisions: same as background oracle above
1400×900 PNG sha256 af6a605251682df2c849b58b45b2d9b453d0931e18853bce3bee7d41e1b9143d
480×270 PNG sha256 24ddf3c8483819a25850d676dbc029f6246c1731371ddf2830cfc1d745fadc73
```

```sh
flutter test test/closed_graph_oracle_test.dart --update-goldens
```

The Rust gates compare decoded grid, bridge, event halo/core/glint, Manrope
title, and JetBrains Mono date pixels. Mean-channel budgets are 0.07 at
1400×900 and 0.22 at 480×270, with changed-pixel ceilings. Bundled-font
wrapping and paragraph widths/heights are asserted separately: host font
rasterizers vary individual hinted glyph-edge channels even when their layout
and global raster errors agree. The scaled allowance accounts for
Flutter-engine/direct-Skia coverage, not world-geometry drift.

`artifact-graph-open-1400x900.png` and
`artifact-graph-half-1400x900.png` fix the same expandable event at animation
progress 1 and 0.5. They cover source-derived layout, eased radial motion,
tethers, chromatic halos, glass fills, marker rings, provenance ticks/arcs,
artifact text, shrinking hub, and fading event labels:

```text
Open PNG sha256 4b69e26368e45977e58430d2b61425d34bc9b17e12dcd29a45f8c8f01e309432
Half PNG sha256 41e948a18eff7f1b032df768ed09e637b4162d1477660c956e99d827a44fbd9b
```

The open/half Rust budgets respectively cap mean channel delta at 0.08/0.075
and changed-pixel fractions at 0.077/0.073. A low mean did not excuse a
displaced glyph: the 0.449-pixel pre-layout-centering error raised the open
mean to 0.134, so the gate failed before placement switched to SkParagraph's
realized width. Structural assertions separately fix the report paragraph at
119×55 pixels, 75.6 intrinsic width, and its source-derived line breaks.

`artifact-neighbor-open-1400x900.png` composes expansion with Flutter's
neighbor collision solver and active-bridge glow/dash treatment (sha256
`f91c91c24da83951ed3b43afa1dd1e09b106914a3bc90c4ce64cbb50a1335693`).
Its Rust gate caps mean drift at 0.08 and changed pixels at 0.085; a separate
topology experiment proves distant and active positions remain fixed while a
colliding neighbor moves.

`artifact-neighbor-midpoint-1400x900.png` fixes the complete scene at 110 ms
of Flutter's 220 ms activation: ease-out-cubic expansion, layout interpolation,
neighbor displacement, and bridge emphasis share one clock. Its sha256 is
`0989cbd34c05ce962a3169f521bb1e8e32e76b051788fdbc97ebfa5ec76c692c`;
the Rust gate caps mean drift at 0.08 and changed pixels at 0.085. This temporal
oracle prevents independently correct endpoints from concealing a wrong path.

The four `full-screen-*-1280x800.png` frames isolate Flutter's fixed record/
zoom chrome and active-event metadata by recording each overlay with and
without its base scene. `buildCanvasFullScreenOracle` is a testing-only aperture
in `lib/main.dart`: it composes the existing private painter and widgets but
initializes no audio plugin, network client, or graph writer. Naming the
unchanged application theme prevents a copied test composition from becoming a
second design implementation. The oracle binding retains automated fake time
and fixed viewport semantics while enabling real application shadows, which
Flutter tests otherwise replace with opaque silhouettes; Manrope, JetBrains
Mono, and Material Icons are loaded explicitly.

```text
Closed + chrome sha256 e4238636fd48f92b5c9c2f42cd79ebfc4db4bb7da9df621e669fbb63e58414bc
Closed base sha256     b854468e83acc9a5d58686838056fe36ccf1fc298eb6420762ebc2ab790a2b9f
Active + metadata      24c0cf7703bfedb749be3ac8a2dfd8f3576988e3d53a98a7afda272975dc30e9
Active base            d129ca171f1160d776921ab97185b02f712853ee0be585d757f7f05d18952a36
```

```sh
flutter test test/full_screen_oracle_test.dart --update-goldens
```

Rust compares overlay residuals over the identical Flutter base. Separate
budgets bind record, control strip, opaque metadata fill, shadows, and text;
the metadata paragraph additionally fixes all five line ranges and baselines.
This keeps platform-sensitive glyph edges from weakening geometry checks.
