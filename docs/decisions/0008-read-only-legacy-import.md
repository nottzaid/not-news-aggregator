# ADR 0008: Import legacy graphs read-only into pristine state

- Status: accepted
- Date: 2026-07-14
- Governs: Flutter-era data continuity, drag-and-drop, startup automation,
  import failure

## Problem

Preserving Git history does not preserve a researcher's corpus. Opening a
Flutter-era database as the Rust application's live file would migrate the only
copy in place; silently merging it into later Rust research would make identity,
revision, and undo provenance ambiguous.

## Decision

The empty window accepts a dropped legacy `graph.sqlite`; installers and
recovery scripts may invoke the same path with `--import-legacy`, optionally
selecting a destination with `--database`. The source is opened read-only,
decoded through the tolerant legacy reader, normalized with generation zero for
pre-generation placements, and validated before the destination reserves its
writer.

Import requires a pristine schema-v3 destination: no events, placements,
history, research sessions, output, clears, or prior revision. Events, bridges,
aliases, placements, generations, and revision then commit in one transaction
and are reloaded for exact comparison before success is shown. The source is
never migrated, copied over, or deleted.

## Rejected alternatives

- In-place migration makes experimentation alter the archival corpus.
- Automatic merge cannot explain collisions between aliases, source identity,
  placement generations, or revisions.
- A GUI toolkit solely for a file picker expands platform and packaging surface;
  native window file-drop plus an automatable flag covers interactive and
  managed recovery without another runtime.
- Silently importing a discovered path mistakes filesystem proximity for user
  intent.

## Evidence required

Acceptance requires byte-identical source retention, exact normalized graph
round-trip, rejection after any destination history, injected mid-import
rollback, application state replacement only after commit, and both preserved
reference databases imported into temporary fresh stores. Release acceptance
must repeat import through the packaged window and command line on Windows and
Linux, including paths with spaces and malformed or locked sources.
