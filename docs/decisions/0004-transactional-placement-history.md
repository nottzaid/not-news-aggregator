# ADR 0004: Commit placement through an append-only reversible log

- Status: accepted for placement concurrency and schema-v1 migration; journal
  topology superseded by ADR 0011
- Date: 2026-07-14

## Problem

Legacy storage has positions and a graph revision but no per-placement
generation or durable inverse. A crash between placement, revision, and history
writes could expose partial success; graph-wide optimistic locking would reject
unrelated moves yet still miss placement ABA after undo.

## Decision

Schema version 1 adds `placement_versions` and `mutation_log` without changing
legacy graph tables. Migration takes a SQLite online backup under a reserved
writer, verifies integrity, then creates both tables and advances
`user_version` in one transaction. Unknown future schemas are rejected.

Each move runs inside `BEGIN IMMEDIATE` and atomically writes placement,
per-event generation, graph revision, and immutable log row. An operation ID
makes an exact retry idempotent and conflicting reuse an error. Stale placement
generations lose even when their graph snapshot was once current.

Undo and redo append rows. Replay reconstructs both stacks; a new move abandons
only the effective redo branch. Before reversal, the store verifies that the
target row's committed placement and generation still match durable state.
Placement history changes no event, alias, artifact, bridge, or semantic field.

## Rejected alternatives and evidence

Writing `placements` alone supplies neither atomic acknowledgement nor a guarded
inverse. A graph revision creates false cross-node conflicts. Mutable history
erases abandoned branches. Copying a live SQLite file can omit WAL state;
SQLite's backup API defines the snapshot.

Tests cover legacy preservation, backup integrity, duplicate operation IDs,
stale writers, restart-spanning undo/redo, redo invalidation, and rollback after
an injected failure between writes. Replace this design only with a store that
proves the same atomicity, per-placement conflict detection, idempotency,
immutable audit, and reversible migration with less operational risk.
