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
title, and JetBrains Mono date pixels. Current budgets are mean/max channel
delta 0.07/5 at 1400×900 and 0.22/4 at 480×270, plus changed-pixel ceilings;
the scaled allowance accounts for Flutter-engine/direct-Skia coverage, not
world-geometry drift.
