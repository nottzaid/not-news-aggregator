# Detach and reconciliation

Status: the first event-drag slice is implemented on the current branch. This
document separates its enforced contract from the larger interaction still to
be built.

The gesture has one grammar:

```text
lift → carry → drop
```

The destination is immediate, user-authored, and authoritative. The origin is
settled asynchronously. The latency belongs to semantic settlement, never to
manipulation.

## Why placement must persist

The original layout equated clusters with connected components and regenerated
every base position on each graph update. The saved baseline disproved that
equivalence: 71 events formed nine components, yet visually distinct research
communities remained meaningfully bridged; 24 events were articulation points,
and 39 of 85 relationships were graph-theoretic bridge edges.

Dragging one event through that global generator would move unrelated work,
allow one destination edge to reshape the Canvas, and pull the event away from
the researcher's cursor. The product therefore persists world position and pin
state. Clusters remain emergent; no `Cluster` entity is introduced.

## Implemented slice

### Pointer state

- Pointer-down on an event arms a drag; empty Canvas still arms camera pan.
- Six screen pixels separate click from drag.
- Once dragging, the event follows local Flutter state. Pointer movement does
  no HTTP, SQLite, JSON, layout regeneration, or model work.
- A target inside the zoom-adjusted magnetic radius is exact: the UI highlights
  that event and previews the bridge.

Artifact dragging, keyboard cancellation, multi-selection, and central-dwell
merge are not implemented.

### Synchronous drop

`POST /graph/drag-transactions` receives:

```json
{
  "eventId": "event-a",
  "originX": 10,
  "originY": 20,
  "destinationX": 300,
  "destinationY": 400,
  "targetEventId": "event-b",
  "expectedRevision": 7
}
```

Under one `BEGIN IMMEDIATE` transaction, the backend:

1. rejects a stale revision, missing event, self-target, or missing target;
2. snapshots the old placement and every incident relationship;
3. writes and pins the new world position;
4. creates an optional `User-curated relationship` with `provenance=user`;
5. records a drag transaction and increments the graph revision.

No LLM work precedes the response. The Flutter client optimistically renders
the destination, then replaces it with the authoritative snapshot.

### Origin reconciliation

The backend starts a separate asyncio task after commit. Its context contains
only the dragged event, old relationships, old neighbors, destination event,
and allowed actions. Hermes must decide every old relationship exactly once:

- `keep`
- `remove`
- `amend` with a nonempty label

Unknown, duplicate, omitted, or invalid bridge actions fail validation. The
new destination bridge is absent from the allowlist and cannot be touched.

The subprocess uses `--toolsets clarify --ignore-rules`, no approval bypass,
four turns by default, and a 45-second timeout. Timeout or task cancellation
kills and reaps the child. No terminal, file, browser, web, delegation, or
mutation tool reaches this model call.

If Hermes is disabled, fails, times out, or emits invalid JSON, deterministic
fallback removes every old relationship in scope. The destination placement
and protected bridge remain.

### Visible settlement

While the client polls the transaction every 240 ms:

- a pulsing afterimage marks the origin;
- old bridges terminate at that afterimage rather than pretending settlement;
- the dropped event remains pinned at the destination.

Resolved or fallback snapshots replace the pending topology and dismiss the
afterimage. Incremental origin-only layout settlement is not yet implemented;
unpinned positions still pass through the existing generator when a snapshot is
applied.

### Destination review and undo

A targeted drop exposes a closable **LET HERMES CHECK THIS** panel. Review is a
separate model request returning an advisory summary. It cannot change the
user-authored bridge, and the current UI does not yet present an evidence list,
proposed diff, accept action, or later contextual re-entry.

`POST /graph/drag-transactions/{id}/undo` restores old incident relationships
and the prior placement, removes the newly created relationship, increments the
revision, and marks the transaction undone. The API is implemented and tested;
the Canvas has no undo control yet.

## Persistence

SQLite now contains:

```text
placements(event_id, x, y, pinned)
graph_meta(key='revision', value)
drag_transactions(id, status, base_revision, committed_revision, payload, plan)
bridges(id, payload)
```

Bridge IDs derive from endpoints and normalized label. Stored graph SSE emits
events, bridges, placements, then `graph.revision`; the client reducer preserves
all four across updates.

Current transaction status is `pending`, `resolved`, `fallback`, or `undone`.
The recorded payload contains enough pre-drop state for one transaction undo,
but this is not yet a general mutation log.

## Enforced invariants

- A stale client revision cannot commit a drop.
- A destination is an exact event or empty space, never an ambiguous community.
- The model can decide only relationships captured before the drop.
- The model cannot delete events or address the destination bridge.
- A valid plan decides every allowed bridge once.
- The new position remains pinned through success and fallback.
- Failure has a deterministic visible result.
- Local pointer motion never awaits semantic work.

## Remaining correctness work

The first slice does not yet satisfy the full design. In priority order:

1. **Settlement conflict control.** Reconciliation validates transaction scope
   but does not compare the current graph revision with its committed revision
   before applying a late plan. A later mutation can therefore make an old plan
   stale.
2. **Concurrent transaction ownership.** Undo restores the dragged event's
   incident edges from its snapshot; it needs conflict rules before multiple
   overlapping drags can be considered safe.
3. **General mutation history.** Add inverse operations, redo, restart recovery,
   idempotency keys, and a visible undo affordance.
4. **Persistent result channel.** Polling works for one client but a graph-wide
   mutation stream should deliver reconciliation and undo across clients.
5. **Incremental layout.** Settle only the affected origin neighborhood while
   preserving unrelated unpinned events.
6. **Bounded semantic API.** The current tool allowlist prevents mutation; a
   future first-class `simulate_origin_plan` / `submit_origin_plan` interface
   should replace free-form JSON output.
7. **Destination evidence.** Store verdict, evidence, and proposed diff; require
   explicit approval for any edit.

## Future interaction

### Artifact promotion

Dragging an expanded source artifact should create a new event only after the
backend validates URL uniqueness and performs one atomic transaction:

- remove the artifact from its parent;
- create the event with source provenance;
- place and pin it;
- optionally create a destination relationship;
- enqueue origin reconciliation;
- record the inverse.

Creating a node before removing the artifact would expose the same URL twice
and violate the current graph contract.

### Event merge

A long central dwell may eventually signal merge, but it needs a visibly
different preview from edge attachment and an explicit confirmation. Merge must
choose canonical title, date, summary, sources, aliases, placement, and incident
relationships; deduplicate URLs and self-loops; and preserve a complete inverse.
It is deliberately later than artifact promotion.

Multi-event dragging is outside the present scope.

## Failure semantics

The contract rejects ambiguity rather than hiding it:

- API rejection restores the authoritative graph stream.
- Model failure produces deterministic detach and an explicit status.
- Restart recovery must eventually resolve or fall back any persisted `pending`
  job; that recovery is not implemented yet.
- Undo during pending reconciliation requires cancellation or supersession; the
  current backend marks undone, while the late task sees a non-pending status and
  returns without applying its plan.

## Verification

Current automated evidence covers:

- immediate placement, pin, protected destination, and revision increment;
- rejection when reconciliation names the protected bridge;
- deterministic fallback and undo restoration;
- stale revision rejection at drop time;
- parsing quiet Hermes JSON;
- reconciliation command isolation from mutating and research tools;
- placement and revision reduction in Flutter;
- existing layout, hover, artifact, and stream behavior.

Run it with:

```sh
flutter analyze
flutter test
UV_CACHE_DIR="$PWD/.uv-cache" uv run --project backend pytest backend/tests
```

Still required before calling the full interaction complete:

- widget tests for click/drag arbitration, pan exclusivity, zoom conversion,
  cancellation, target preview, pending/fallback rendering, and undo;
- conflict tests for late reconciliation, overlapping drags, and restart;
- golden tests for afterimage and review UI at 35%, 45%, 100%, and 280% zoom;
- frame-time measurement under a 16.7 ms budget;
- hands-on tests for leaf, articulation event, cross-community drop, fallback,
  undo, and unrelated Canvas use during reconciliation.

The perceptual acceptance criterion remains the architectural one: delaying
Hermes must never change how the event follows the hand.
