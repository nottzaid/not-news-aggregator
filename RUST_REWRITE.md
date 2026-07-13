# Rust rewrite ledger

Durable recovery state for the Rust rewrite; not a diary.

## Maintenance contract

- After compaction, read this before acting.
- In the same change, record altered invariants, decisions, contracts,
  evidence, risks, and checkpoints.
- Keep **Current checkpoint** sufficient to resume without chat reconstruction.
- Before a PR or release, reconcile this with code, issues, ADRs, and checks.

## Mandate

Rewrite the Linux research canvas in Rust under [research issue
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
- Text/voice question → Hermes-sourced events, relationships, and progress.
- Preserve artifacts, semantics, placement, camera, hover expansion, and source
  opening unless a recorded decision replaces them.
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
  2026-07-14. One Linux desktop process calls renderer-independent
  `domain`/`store`/`agent` crates; eframe/wgpu is provisional; SQLite remains
  durable; Hermes, SearXNG, Browse.sh, providers, Kokoro, and browser remain
  external. Pointer state is ephemeral; durable state changes on commit.
  Movement is placement-only; agent failure is non-destructive. Replace the
  renderer on measured failure; restore a process boundary only for an
  independently operated client/service.

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
  pass; six active workspace tests pass; the separately invoked ignored test
  decodes both preserved databases read-only and asserts their exact counts.
- Rust 1.95 is the minimum: rusqlite 0.40.1/libsqlite3-sys 0.38.1 uses
  `cfg_select!`, stabilized in 1.95; an executable 1.92 probe rejected the
  earlier declaration. `rust-toolchain.toml` pins 1.95.0 plus rustfmt/Clippy.
- Adversarial domain cases found and closed placement-generation ABA after
  restoring absence, plus partial mutation on revision overflow. Generated
  store data proves dangling bridge and placement rows remain hidden rather
  than invalidating an otherwise readable legacy snapshot.

## Open decisions

- Exact SQLite migration/backup strategy.
- Whether eframe satisfies measured canvas tails before a lower-level renderer
  is warranted.
- Audio capture crate and Linux packaging boundary.
- Which existing transient UI behaviors merit exact parity versus principled
  replacement; record each nontrivial choice in an ADR.

## Current checkpoint

- Date: 2026-07-14.
- Branch: `rust-rewrite`, created from
  `3220d3af5607d27b8d945026f8c0551921a4addc`.
- Phase: architecture and compatibility kernel verified; UI remains a shell,
  not a vertical slice.
- Accepted decision: ADR 0001 fixes the single-process Rust boundary and
  provisional eframe renderer; its alternatives and reversal conditions are
  indexed above.
- SQLite evidence: backup has 71 events/85 bridges/9 aliases and only legacy
  tables; live has 71/81/9, two placements, two fallback transactions, revision
  4. Both pass integrity checks; exact hashes live in the inventory.
- Verified Rust contracts: `MoveNode` cannot touch graph semantics; guarded
  inverses reject later generations, including absent-placement ABA; corrupted
  inverses and overflow errors are atomic; legacy reads are read-only,
  shape-tolerant, and preserve snapshot filtering of dangling rows. Formatting,
  six active tests, the ignored real-database test, and warning-denied Clippy
  pass.
- History invariant confirmed: preserve all Flutter commits and existing tags;
  do not force-push or change the default branch as incidental rewrite work.
- Next safe action: commit and publish this bounded kernel through a draft PR;
  then render the live read-only graph before introducing writable migration.
