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
