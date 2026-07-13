# Rust rewrite ledger

Durable recovery state for the Rust rewrite; not a diary.

## Maintenance contract

- After compaction, read this before acting.
- In the same change, record altered invariants, decisions, contracts,
  evidence, risks, and checkpoints.
- Keep **Current checkpoint** sufficient to resume without chat reconstruction.
- Before a PR or release, reconcile this with code, issues, ADRs, and checks.

## Mandate

Rewrite the Windows/Linux research canvas in Rust under [research issue
#1](https://github.com/muradkant/not-news-aggregator/issues/1) and
[implementation issue
#2](https://github.com/muradkant/not-news-aggregator/issues/2). Preserve useful
behavior and data; correct drag semantics; collapse accidental local process
boundaries rather than transliterating Dart and Python.

## History preservation

- Never rewrite Flutter history, force-push the rewrite, or delete/retarget
  `v1.0`, `v1.1`, `v1.2`, their commits, branches, or releases.
- Deleting Flutter/Python files on the Rust branch does not retire their history.
- Default-branch change, Flutter retirement, or Rust major release requires
  parity/migration evidence and explicit user approval.

## Product truth to preserve

- Persistent visual research graph, not feed reader.
- Windows and Linux are the product platforms. Do not spend architecture,
  verification, packaging, or maintenance effort on macOS, mobile, or web.
- Text/voice question → Hermes-sourced events, relationships, and progress.
- Replicate Flutter's visible composition, typography, color, geometry,
  controls, panels, transitions, and animation timing as the design oracle;
  preserve artifacts, semantics, placement, camera, hover expansion, and source
  opening unless a recorded decision replaces behavior.
- Existing data survives exact compatibility or verified reversible migration.
  Research may be slow; carrying may not be.

## Intentional correction

Movement, relation, and semantic revision become distinct commands:

- `MoveNode` changes only world position and pin state.
- `CreateEdge` is an explicit relation gesture with an exact target and
  meaningful predicate.
- `DetachEdges` or rewiring starts from an explicit semantic affordance and
  previews a reversible diff.
- Hermes may advise or submit a bounded proposal; model failure preserves graph
  knowledge and leaves the proposal unresolved.

No ordinary move invokes Hermes, creates an edge through proximity alone, or
deletes relationships. Rust is expected to improve architecture and tooling,
but this semantic correction—not the language—is what restores direct
manipulation.

## Decision index

Compress every accepted ADR here; read it before changing its subject.

- [ADR 0001: Collapse the local application boundary in
  Rust](docs/decisions/0001-rust-application-boundary.md) — accepted
  2026-07-14. One desktop process calls renderer-independent
  `domain`/`store`/`agent` crates; SQLite remains durable; Hermes, SearXNG,
  Browse.sh, providers, Kokoro, and browser remain external. Pointer state is
  ephemeral; durable state changes on commit. Movement is placement-only;
  agent failure is non-destructive. ADR 0002 supersedes its original
  Linux-first eframe clauses. Restore a process boundary only for an
  independently operated client/service.
- [ADR 0002: Port Flutter painting through direct Skia surface
  adapters](docs/decisions/0002-direct-skia-renderer.md) — accepted 2026-07-14;
  supersedes ADR 0001's renderer/platform clauses. No general GUI framework:
  platform-neutral state builds immutable scenes painted by direct Skia;
  Linux begins with glutin/GL, Windows gets a separately verified adapter.
  Cache geometry/text and bound damage; bundle fonts; confine audited unsafe to
  GPU/window setup; release users never compile Skia. Reverse only for a shell
  that preserves direct painting, bounded damage, raw input, accessibility,
  output, and timing while removing more risk than it adds.
- [ADR 0003: Ship native Rust artifacts and diagnose external
  capabilities](docs/decisions/0003-release-delivery-contract.md) — accepted
  2026-07-14. New releases originate only from the Rust line; Flutter/Python
  remain archival and are never packaged. Windows/Linux each receive an
  integrated and relocatable artifact containing the optimized app and owned
  assets, with program/data paths separated. The saved canvas works without
  external research services; missing capabilities are diagnosed. Stable
  releases require native builds, hashes/inventory/signing, safe migration,
  clean-machine install/upgrade/failure-recovery/uninstall evidence, and
  preservation of user research. Format/tool selection remains empirical.

## Implementation order

1. Inventory observable behavior, SQLite data, environment contracts, and
   recoverable fixtures.
2. Establish pure Rust domain types, command semantics, invariants, and
   old-format readers.
3. Render the saved graph with pan, zoom, hover, source opening, and move-only
   dragging; persist placement and expose undo.
4. Add an append-only mutation log, redo, crash recovery, idempotency, and
   per-entity conflict rules.
5. Port research-event ingestion and visible Hermes progress without a loopback
   HTTP/SSE boundary.
6. Port text/voice entry and external search/runtime orchestration.
7. Add explicit relation and detach flows, bounded advisory proposals, and
   evidence-backed acceptance.
8. Add artifact promotion; defer destructive merge until every prior invariant
   has executable evidence.

Keep Flutter runnable as oracle until data round-trips and preserved behavior
has executable replacement evidence.

## Verification doctrine

A check has weight only if its experiment could expose the claimed failure.

Required evidence includes:

- synchronized Flutter/Rust captures for canonical viewports, hover states,
  panels, recording/research states, drag phases, and animation timestamps;
- image-difference thresholds tight enough to expose altered geometry, color,
  typography, clipping, or easing, with every intentional delta reviewed;
- differential decoding and rendering inputs from existing saved graphs;
- schema migration round-trip plus backup restoration;
- command-model properties: scope, inverse fidelity, idempotency, stale-result
  rejection, and preservation of unrelated graph state;
- generated graph cases: singleton, leaf, chain, cycle, articulation point,
  high-degree hub, sparse bridge between dense communities, and parallel
  semantic edges;
- scripted pointer traces across zoom levels, cancellation, lost focus,
  multiple devices, and edge-of-window movement;
- frame and input-to-present distributions on the 71-event reference graph,
  with p99 carrying frames at or below 16.7 ms on the reference machine;
- forced Hermes absence, malformed output, timeout, cancellation, restart, and
  late completion without loss of accepted graph knowledge;
- crash-at-each-transaction-boundary recovery and repeated undo/redo;
- release-build smoke operation from a clean environment.
- clean Windows/Linux install and relocatable launches; platform-native data
  paths; prior-version upgrade; injected migration rollback; restart; uninstall
  preserving the graph; artifact hash/inventory verification; absence of build
  toolchains and repository-relative runtime assumptions.

Publish these experiments as GitHub Checks; checks are not unit-test synonyms.

## Engineering trail protocol

- **Issue:** falsifiable problem/outcome, evidence, constraints, acceptance.
- **ADR under `docs/decisions/`:** consequential decision, alternatives,
  trade-offs, and reversal conditions.
- **Draft pull request:** bounded implementation linked to its issue; state the
  governing interpretation, exact change, evidence, risks, and omissions.
- **Review:** adversarial diff examination. An agent self-audit is labeled as
  such and never presented as independent approval.
- **Checks:** reproducible experiments attached to the exact commit.
- **Release:** reproducible usable state, migration notes, known limitations,
  and artifacts.

Create no artifact without unique information. Add a Project only when
concurrency obscures state; crystallize Discussion conclusions into issue/ADR.

## Current evidence and repository state

- Baseline branch/commit at rewrite start:
  `experimental-optimization` / `3220d3af5607d27b8d945026f8c0551921a4addc`.
- Drag implementation: `5ed1bc9aaaa0363584134f75aeb8c9d705e4cbea`.
- Issues: #1 holds diagnosis; #2 holds Rust delivery and acceptance.
- Baseline verification observed on 2026-07-13: `flutter analyze` passed; all
  19 Flutter tests passed; all 47 backend tests passed. Those tests do not
  establish drag smoothness or the missing concurrency/recovery properties.
- `rust-contracts.yml` publishes formatting, warning-denied lint, command-model,
  generated compatibility evidence; pointer, performance, crash, migration,
  and ignored local-database experiments remain outside CI.
- Rust scaffold evidence on 2026-07-14: formatting and warning-denied Clippy
  pass; the separately invoked ignored test decodes both preserved databases
  read-only and asserts their exact counts.
- Rust 1.95 is the minimum: rusqlite 0.40.1/libsqlite3-sys 0.38.1 uses
  `cfg_select!`, stabilized in 1.95; an executable 1.92 probe rejected the
  earlier declaration. `rust-toolchain.toml` pins 1.95.0 plus rustfmt/Clippy.
- Adversarial domain cases found and closed placement-generation ABA after
  restoring absence, plus partial mutation on revision overflow. Generated
  store data proves dangling bridge and placement rows remain hidden rather
  than invalidating an otherwise readable legacy snapshot.
- Renderer substrate research on 2026-07-14 found current, serious precedent
  for the proposed low-level stack: Alacritty uses winit/glutin; Slint's Skia
  renderer uses `skia-safe` plus glutin, selecting Direct3D on Windows and
  GL/Vulkan on Linux. Approximate crates.io totals were 45.7M winit, 27.8M
  glutin, and 3.1M `skia-safe` downloads. This establishes adoption, not fit.
- A disposable `skia-safe` 0.99 spike built and ran: both bundled fonts decoded
  and registered; SkParagraph accepted Manrope; quadratic paths measured; blur
  masks and radial-gradient shaders instantiated. Its feature combination
  missed the upstream binary cache because it unnecessarily enabled embedded
  FreeType and compiled roughly 1,450 C++ objects. Vizia's supported Skia feature
  set hit the cache and built cleanly in about 34 seconds. Normal builds must use
  cache-supported features; CI retains a source-build fallback. This proves
  primitive availability, not parity.
- A released Vizia 0.4 counter-spike compiled, presented direct Skia output at
  1.5× scale, accepted bundled Manrope as a resource, and exposed physical
  pointer/capture paths. Source inspection found full-view invalidation for the
  required custom canvas and a released Skia 0.93 dependency while active
  development uses 0.99. ADR 0002 rejects Vizia because exact custom painting
  bypasses its main value while retaining those constraints.
- The accepted boundary is implemented: `not-news-platform` owns winit/glutin
  lifecycle, GL presentation, Skia surface wrapping, scale, resize, and frame
  deadlines; its three required unsafe sites are local context/surface/
  framebuffer calls. `not-news-renderer` owns the source-derived palette,
  Flutter cubic evaluator, unbounded viewport transform, deterministic
  background, grid, and deterministic legacy layout. Eframe/egui no longer
  occurs in the application dependency graph.
- Renderer evidence: nine tests pass, including exact Flutter curve samples,
  deterministic grain/PNG output, large-coordinate viewport inversion and grid
  rasterization, placement precedence, collision separation, and deterministic
  layouts. Renderer, platform, and app pass warning-denied Clippy. The native
  adapter is compile-verified but intentionally has not displayed the
  incomplete app; this is not on-window, parity, driver, damage, or timing
  evidence.
- The 2026-07-14 workspace checkpoint passes formatting, warning-denied Clippy,
  and all 15 active tests; the one real-database experiment remains explicit
  and ignored by default. CI now names the broader domain/decoding/renderer
  contract it executes rather than implying every test concerns movement. A
  Windows runner now compiles every workspace target at the declared minimum
  toolchain.
- A Linux-hosted `x86_64-pc-windows-msvc` check compiles `renderer` and
  `platform`, including Skia, winit, and glutin. Checking the whole workspace
  from Linux correctly stopped at bundled SQLite because MSVC's `lib.exe` is
  absent; only the native Windows CI job may close that application/store build
  gap. Neither result proves Windows execution or visual parity.
- `cargo-xwin` builds the complete MSVC workspace on Linux. The reproducible
  hidden Wine 11.11 smoke creates the Windows window and GL/Skia surface, paints
  background/grid, presents once, and exits zero; its first form timed out
  because invisible windows received no redraw, exposing and closing a hidden-
  lifecycle assumption. Wine reported EGL driver fallback warnings. This proves
  emulated Windows initialization/presentation, not native driver, installer,
  or parity behavior.
- The Linux optimized binary links Skia and SQLite into the executable. Its ELF
  imports are only `libstdc++`, `libgcc_s`, `libm`, `libc`, and the loader; GUI/
  GL libraries are discovered at runtime. The first clean build linked in about
  two minutes on the reference machine. Package experiments must establish a
  compatible glibc floor and graphics/runtime diagnostics rather than assuming
  this development host's libraries.

## Open decisions

- Exact SQLite migration/backup strategy.
- Exact Linux/Windows Skia backend selection and fallback after on-window,
  driver, device-loss, fidelity, and packaging evidence. ADR 0002 fixes the
  direct painter and thin-adapter boundary, not an unmeasured backend forever.
- Audio capture crate and Linux packaging boundary.
- Exact installer/portable formats, packaging orchestrator, signing provider,
  supported Linux ABI floor, and whether any external research capability can
  meet ADR 0003's ownership bar for managed installation.
- How to eliminate renderer-specific rasterization variance without weakening
  visual and temporal parity requirements.

## Current checkpoint

- Date: 2026-07-14.
- Branch: `rust-rewrite`, created from
  `3220d3af5607d27b8d945026f8c0551921a4addc`.
- Phase: direct-renderer substrate and compatibility kernel verified; UI is an
  incomplete paint-port slice, not a design candidate or vertical slice.
- Accepted decisions: ADR 0001 fixes the single-process Rust boundary; ADR 0002
  supersedes its provisional eframe/Linux clauses with a direct Skia painter and
  thin Windows/Linux surface adapters. Eframe and Vizia are rejected for this
  application by measured fidelity/control/invalidation mismatch, not obscurity.
- Delivery invariant: ADR 0003 makes clean Windows/Linux packaging, safe
  upgrades, degraded-capability startup, and research-preserving uninstall part
  of product acceptance. Rust releases never package archival Flutter/Python.
- SQLite evidence: backup has 71 events/85 bridges/9 aliases and only legacy
  tables; live has 71/81/9, two placements, two fallback transactions, revision
  4. Both pass integrity checks; exact hashes live in the inventory.
- Verified Rust contracts: `MoveNode` cannot touch graph semantics; guarded
  inverses reject later generations, including absent-placement ABA; corrupted
  inverses and overflow errors are atomic; legacy reads are read-only,
  shape-tolerant, and preserve snapshot filtering of dangling rows.
- History invariant confirmed: preserve all Flutter commits and existing tags;
  do not force-push or change the default branch as incidental rewrite work.
- Visual invariant clarified: Flutter is the exact appearance/animation oracle;
  the first Rust smoke frame and Vizia probe were diagnostic scaffolding and
  fail parity. Never present a partial renderer as product output; use offscreen
  checks until its claimed state is comparable.
- Platform scope: Windows and Linux only; Linux is the first executable oracle,
  while Windows build/package/backend evidence is required before parity.
- Current code: the app reads the legacy graph, resolves deterministic world
  positions, and invokes the direct surface adapter; only Flutter's background
  and grid layers are painted. The binary was compiled and linted, not launched.
- Next safe action: build a Flutter-generated offscreen/canonical capture oracle,
  then port bridges, events, text, panels, and interaction state in Flutter
  paint order with image/time deltas before any writable migration.
