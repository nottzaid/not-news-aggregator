# ADR 0004: Commit placement with an append-only reversible log

- Status: accepted for placement concurrency and schema-v1 migration; history
  topology superseded by ADR 0011
- Date: 2026-07-14
- Governs: local graph mutation, compatibility migration, undo/redo, crash and
  concurrency behavior

## Problem

The legacy database stores positions and a graph-wide revision but no
placement generation or durable inverse. Writing the pointer's optimistic
state directly would make a crash between placement, revision, and history
writes observable; graph-wide revision alone would also reject unrelated work
while failing to detect placement ABA after undo.

## Decision

Schema version 1 adds `placement_versions` and `mutation_log` without changing
legacy event, bridge, alias, placement, or metadata tables. Migration takes a
SQLite online backup under a reserved writer lock, verifies
`PRAGMA integrity_check`, then creates both tables and advances
`user_version` in one transaction. A future schema is rejected rather than
guessed.

Every move loads the current snapshot inside `BEGIN IMMEDIATE`, applies domain
`MoveNode`, and atomically writes its placement, per-event generation,
graph revision, and immutable log row. The caller supplies an operation ID;
an identical retry returns its original sequence without another mutation,
while reuse for another payload fails. Stale placement generations lose even
when their graph snapshot was once current.

Undo and redo are new log rows, not edits to history. Replaying `move`, `undo`,
and `redo` rows reconstructs both stacks; a new move clears only the replayed
redo branch. Each reversal verifies that the targeted row's committed
placement and generation still equal durable state before applying its stored
prior placement through the domain inverse. No history operation touches an
event, bridge, alias, artifact, or semantic field.

## Rejected alternatives

- Updating `placements` alone cannot distinguish durable success from a crash
  before revision/history and supplies no guarded inverse.
- Using only the graph revision creates false conflicts between independent
  nodes and does not close placement ABA.
- Mutating or deleting history rows makes forensic recovery depend on the
  newest write and erases abandoned redo branches.
- Copying the live SQLite file as a backup can omit WAL state or capture a
  torn concurrent view; SQLite's backup API defines the snapshot.
- Migrating on first release without a verified pre-schema artifact makes
  rollback a promise rather than an operation.

## Consequences and evidence required

The legacy Python reader remains compatible because its tables retain their
shape. Rust owns all writes after migration; concurrent legacy writers cannot
maintain placement generations and must not share a writable production file.
The sidecar backup is retained until explicit retention policy exists.

Acceptance requires temporary-database experiments for byte-significant legacy
content preservation, backup integrity, duplicate operation IDs, stale
writers, restart-spanning undo/redo, redo invalidation, and injected failure
after placement write but before log append. Release evidence must additionally
kill real processes at transaction boundaries and restore the backup through
the shipped recovery path.

Reverse this decision only if another store proves the same atomic scope,
per-placement conflicts, idempotency, immutable audit trail, reversible
migration, and legacy-data preservation with less operational risk.
