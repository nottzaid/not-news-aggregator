# ADR 0007: Clear the corpus as one retry-safe transaction

- Status: accepted
- Date: 2026-07-14

## Problem

Clearing only visible findings can leave private prompts, aliases, activity, or
undo material behind. Clearing the UI before durable success can display an
empty canvas while SQLite retains the corpus.

## Decision

Schema version 3 records destructive operation IDs and committed revisions. One
`BEGIN IMMEDIATE` deletes research output and sessions, history, placements,
bridges, aliases, and events; advances the monotonic revision; and records the
operation. An identical immediate retry returns the same empty revision; reuse
after later mutation conflicts. Any failed delete restores every table.

Clear is enabled only while capture, transcription, and research are idle. The
application resets graph, interaction, camera, activity, and prompt only after
commit; failure preserves durable and visible state. Recreating the database is
rejected because it discards migration identity and turns atomicity into a
filesystem replacement protocol.

Evidence covers complete-table erasure, one revision increment, retry identity,
post-commit UI reset, rollback after a late injected failure, and upgrade from
legacy schema fixtures through a verified backup.
