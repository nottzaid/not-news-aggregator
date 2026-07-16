# Immutable reference rasters

These decoded-pixel specimens preserve the visible and temporal contract of the
archived Flutter implementation; they are evidence, not runtime assets or a
second design source. They were generated from commit
`3220d3af5607d27b8d945026f8c0551921a4addc` with Flutter 3.44.1
(`924134a44c`) and engine `c416acfeb8` at DPR 1.

The corpus fixes the grid; event and bridge geometry; bundled Manrope and
JetBrains Mono layout; artifact expansion at endpoints and 110 ms midpoint;
neighbor displacement; record/zoom chrome; activity/status surfaces; and both
1280×800 and narrow 640×720 composition. Rust tests compare decoded pixels,
localized residuals, line ranges, and structural geometry under narrow budgets;
PNG encoder bytes are deliberately outside the contract.

Regeneration belongs on the archival implementation branch and requires review
of both the source change and the resulting Rust delta. Current-branch tests
must never widen a threshold merely to absorb altered geometry, color, timing,
or typography. Exact hashes and per-frame tolerances live beside their consuming
tests in `crates/renderer` so evidence and rejection threshold cannot drift
independently.
