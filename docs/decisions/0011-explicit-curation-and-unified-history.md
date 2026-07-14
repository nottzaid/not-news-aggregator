# ADR 0011: Curate explicit facts through one reversible command history

- Status: accepted
- Date: 2026-07-15
- Governing semantics: [issue #4](https://github.com/muradkant/not-news-aggregator/issues/4)
- Supersedes: ADR 0004's placement-only history topology

## Problem

Dragging expresses spatial arrangement, never epistemic intent. Inferring a
relationship from overlap, proximity, or release position makes an imprecise
motor gesture mutate knowledge. Conversely, adding explicit relate, detach, and
artifact-promotion commands in a second undo stack would make `Ctrl+Z` depend on
mutation type rather than chronology and could restore facts over later work.

## Decision

Schema version 4 rebuilds `mutation_log` as one append-only chronology for
`move`, `relate`, `detach`, `promote`, `undo`, and `redo`. Existing placement
rows retain their operation identity, per-event generation, prior/next
placement, and sequence. Semantic rows store the normalized request plus exact
before/after values for only the touched events, aliases, and bridges. Migration
uses the existing verified-backup protocol; no Flutter-era table changes shape.

Every command receives an operation ID and expected durable revision. An exact
retry returns its original outcome; reuse for another kind or payload fails.
Relate resolves aliases, rejects self-loops and identity collisions, normalizes
a user-written predicate to 96 Unicode scalars, and creates one named bridge
with user provenance. Detach removes one selected bridge ID, never every edge
near a node. Promotion selects one exact artifact URL and either creates a
first-class event or an alias to an event with the same canonical primary URL;
an optional explicit relationship shares that transaction.

Undo and redo append transitions to the same journal. Replay derives one pair
of chronological stacks across restarts; any new command invalidates only the
effective redo branch. Before applying a semantic inverse, the transaction
verifies every touched value still equals the recorded state and validates the
candidate graph. It therefore preserves unrelated later facts and refuses an
inverse that would leave a dangling bridge. Placement inverses retain ADR
0004's per-event ABA guard.

The application exposes these commands through a right-click or `Ctrl+E`
curation surface. Choosing the numbered action and exact target precedes text
entry; movement is disabled during the flow. Dense incident-edge lists paginate
without hiding relationships. Import and clear discard transient curation
state only after durable success.

## Rejected alternatives

- Drag-created bridges conflate layout with meaning and recreate issue #1's
  defect in a faster language.
- Separate semantic and placement undo stacks cannot answer what the user's
  latest action was.
- Snapshot-wide inverses overwrite unrelated work; touched-value transitions
  make the mutation boundary reviewable and conflict-detectable.
- Promotion by copied text loses source identity; exact artifact URL plus
  canonical-primary deduplication preserves provenance.
- Cascading inverse deletes conceal dependency errors; refusing an unsafe undo
  leaves both graph and journal unchanged.

## Evidence required

Acceptance requires contradictory-retry rejection, mixed history across
restart, redo invalidation, exact unrelated-state preservation, alias-resolved
relations, canonical promotion, dangling-inverse refusal, a UI-to-SQLite trace
that performs no drag, bounded Unicode input, and paged selection of every edge
on a dense hub. Hosted Windows and Linux checks must repeat the active suite
against the migrated schema before issue #4 can close.
