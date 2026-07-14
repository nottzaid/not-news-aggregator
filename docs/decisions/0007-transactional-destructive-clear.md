# ADR 0007: Clear the research corpus as one retry-safe transaction

- Status: accepted
- Date: 2026-07-14
- Governs: destructive clear, schema version 3, revision continuity, UI reset

## Problem

“Clear canvas” is destructive, not a paint operation. Deleting events before
activity, placements, or history can leave private prompts or resurrectable
state behind; resetting the camera before commit can visually claim success
after SQLite rejects the deletion.

## Decision

Schema version 3 records destructive operation IDs and committed revisions.
Under one `BEGIN IMMEDIATE`, clear deletes research output, sessions, placement
history, placements, bridges, aliases, and events, then advances the monotonic
graph revision and records the operation. An identical immediate retry returns
the same empty revision; reuse after later mutation conflicts. SQLite rollback
therefore restores every table if any late deletion fails.

The application permits clear only while capture, transcription, and research
are idle. It replaces the in-memory graph, interaction reducer, camera, activity,
and prompt only after commit; failure preserves both durable and visible state.

## Rejected alternatives

- Deleting only visible event rows leaves research prompts, aliases, and undo
  material outside the user's reasonable meaning of “clear.”
- Recreating the database discards migration identity and makes crash-safe
  replacement a filesystem protocol.
- Resetting revision to zero makes stale writers indistinguishable from a new
  corpus.
- A confirmation dialog would change the Flutter interaction contract; durable
  atomicity and explicit disabled states solve corruption, not accidental intent.

## Evidence required

Acceptance requires complete-table erasure, one revision increment, same-ID
retry, application reset after commit, and an injected trigger failure after
earlier deletes proving knowledge, activity, revision, and operation log all
roll back. Packaging must additionally verify upgrade from schema versions 0–2
through a checked backup before exposing clear.
