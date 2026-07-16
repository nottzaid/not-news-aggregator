# ADR 0011: Put explicit curation in one reversible chronology

- Status: accepted; supersedes ADR 0004's placement-only journal topology
- Date: 2026-07-15
- Governing semantics: [issue #4](https://github.com/muradkant/not-news-aggregator/issues/4)

## Problem

Dragging expresses arrangement, never epistemic intent. Inferring a relation
from overlap, proximity, or release position makes imprecise motor input mutate
knowledge. Giving explicit semantic commands a second undo stack would make
`Ctrl+Z` depend on mutation type rather than time and could restore facts across
later work.

## Decision

Schema version 4 rebuilds `mutation_log` as one append-only chronology for
`move`, `relate`, `detach`, `promote`, `undo`, and `redo`. Placement rows retain
operation identity, generation, prior/next position, and sequence. Semantic rows
store a normalized request and exact before/after values only for touched
events, aliases, and bridges. Migration uses the verified-backup protocol and
does not alter Flutter-era table shapes.

Every command carries an operation ID and expected revision. Exact retry returns
its original outcome; conflicting reuse fails. Relate resolves aliases, rejects
self-loops and identity collision, bounds the user's predicate to 96 Unicode
scalars, and records user provenance. Detach removes one selected bridge.
Promotion selects one exact artifact URL, then creates a first-class event or an
alias to the canonical primary URL; an optional explicit relation shares the
transaction.

Undo and redo append transitions. Replay derives one chronological pair of
stacks across restarts; a new command invalidates only the effective redo
branch. Semantic reversal first verifies every touched value and validates the
candidate graph, preserving unrelated later facts and refusing dangling edges.
Placement reversal retains the per-event ABA guard.

The right-click/`Ctrl+E` surface chooses action and exact target before text
entry, disables movement while open, and paginates dense edge lists. Import and
clear discard transient curation only after durable success.

## Evidence

Tests cover conflicting retry, mixed restart history, redo invalidation,
unrelated-state preservation, alias relations, canonical promotion,
dangling-inverse refusal, bounded Unicode, dense pagination, and a UI-to-SQLite
trace that creates a relation without dragging. Reverse only if another model
preserves explicit intent and one crash-safe chronology with less persistent or
interaction complexity.
