# ADR 0008: Import legacy graphs read-only into pristine state

- Status: accepted
- Date: 2026-07-14

## Problem

Git history does not preserve a researcher's corpus. Opening the only Flutter
database as the Rust live store would migrate evidence in place; silently
merging it into later work would make identity, revision, and undo provenance
ambiguous.

## Decision

An empty window accepts a dropped `graph.sqlite`; automation uses
`--import-legacy`, optionally with `--database`. The source opens read-only,
passes the tolerant legacy decoder, receives generation zero for old placements,
and validates before the destination reserves a writer.

Import requires a pristine schema-v4 destination: no graph, revision, journal,
research, or clear history. Events, bridges, aliases, placements, generations,
and revision commit together and reload for exact comparison. The source is
never migrated, overwritten, or deleted. Automatic path discovery and implicit
merge are rejected because filesystem proximity is not user intent and
collision policy would be invented.

Evidence requires byte-identical source retention, exact normalized round-trip,
rejection after any destination history, injected rollback, and packaged
Windows/Linux import through paths containing spaces as well as malformed or
locked sources.
