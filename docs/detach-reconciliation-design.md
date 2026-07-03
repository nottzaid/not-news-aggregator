# Detach and Reconciliation Design

Status: proposed interaction and architecture contract. No feature code has
been implemented from this document.

## Baseline and rollback

The tracked application baseline is commit `519caa1`, protected by the local
branch `codex/pre-detach-design-20260703`.

The saved Canvas is ignored by Git, so it has a separate SQLite snapshot:

`backend/data/backups/pre-detach-design-20260703.sqlite`

The snapshot passes `PRAGMA integrity_check`, contains 71 events, 85 bridges,
and 9 aliases, and has the same SQL dump as the live database at the start of
this design pass.

Pre-existing untracked files under `FEATURE_NOTES.md`, `SKILL.md`, and `docs/`
were not modified during baseline capture.

## Product contract

The visible interaction is one direct-manipulation grammar:

`lift -> carry -> drop`

Clusters remain an emergent result of events, relationships, and layout. This
design does not add a persistent `Cluster` entity.

The interaction is deliberately asymmetric:

- The destination is immediate, user-authored, and authoritative.
- The origin is reconciled asynchronously by Hermes.
- A researcher may explicitly ask Hermes to review the destination. That review
  is advisory and cannot alter the new connection without approval.
- The dragged object never waits for an LLM and never loses contact with the
  cursor.

The latency belongs to semantic settlement, not manipulation.

Asynchronous here means that the synchronous drop transaction returns before a
separate reconciliation job runs. The implementation may use an event-loop
task, subprocess, or worker queue; it does not require an operating-system
thread. The architectural requirement is concurrency: reconciliation cannot
block pointer movement, the drop response, or unrelated Canvas work.

## What the current application implies

The current layout derives formal clusters as connected components of every
bridge. It then generates seeded positions for each component. It does not
persist manual positions, and every graph-state update regenerates all base
positions.

The saved graph demonstrates why a formal component is not the same thing as a
perceived community:

- 71 events form 9 connected components.
- The largest component contains 20 events.
- Cybersecurity and quantum-security communities appear visually distinct but
  are joined by several meaningful bridges.
- 24 events are articulation points.
- 39 of the 85 stored relationships are graph-theoretic bridge edges whose
  removal increases the number of components.

Consequently, a node drag cannot call the existing
`generateBasePositions(...)` after every intermediate mutation. That would
move unrelated events, allow a new destination bridge to reshape the whole
Canvas, and pull the dragged node away from the user's drop point.

The implementation needs persistent placement and incremental settling, not a
stored cluster model.

## Interaction state machine

### 1. Idle

Canvas movement behaves as it does now. Hovering an event keeps the existing
metadata and artifact behavior.

### 2. Armed

Pointer-down is classified before movement:

- Event hit: arm event drag.
- Expanded artifact hit: arm artifact drag.
- Empty Canvas: arm camera pan.

A small movement threshold preserves ordinary clicks. Crossing the threshold
commits to exactly one interaction; a node drag cannot turn into a camera pan.

### 3. Carrying

The node follows the pointer using local Flutter state only. No backend or
Hermes call occurs during pointer movement.

Its old incident bridges remain visible but become slightly desaturated and
tense. This indicates that they have not yet been reconciled.

Potential event targets acquire a magnetic field during the drag. The field
may illuminate a surrounding community, but one exact anchor event is always
highlighted and a preview bridge identifies the precise connection that a drop
will create. The user never drops onto an ambiguous aggregate.

### 4. Dropped

The application synchronously commits:

- The dragged event's world position.
- A pin preserving that position across graph updates.
- The new user-authored destination bridge when an anchor was targeted.
- A graph revision and undo record.
- An origin-reconciliation job containing the pre-drop relationship snapshot.

The new bridge is marked as user-authored and protected from the origin job.
The node and new bridge appear immediately.

Dropping on empty space creates no destination bridge. The event remains at the
drop position while its former relationships are reconciled.

The drop position remains authoritative after reconciliation as well. The
background job may settle the topology left at the origin, but it cannot move
the researcher's dropped event. The event remains manually pinned until the
researcher drags it again.

### 5. Origin reconciling

A restrained semantic afterimage remains at the original position:

- A translucent remnant of the dragged node marks the unresolved origin.
- Former bridge endpoints terminate at that remnant rather than pretending
  that Hermes has already decided.
- A slow orbital sweep indicates active reconciliation.
- The origin neighbourhood remains spatially stable while the job runs.

Hermes evaluates only the relationships that existed before the drag. For each
one, it may propose:

- Keep the relationship.
- Remove the relationship.
- Amend its label or relationship semantics.

When the validated plan commits:

- Removed relationships retract into the afterimage and dissolve.
- Kept relationships extend from the origin towards the event's new position.
- Amended relationships transition without disappearing.
- Only components affected at the origin settle into their new positions.
- The afterimage disappears.

The destination position remains pinned throughout this process.

### 6. Optional destination review

A compact, closable review box appears near the new connection:

`LET HERMES CHECK THIS CONNECTION`

The box does not start Hermes automatically. The researcher may close it or
activate it when uncertain. The same review action remains available later
from the connection's contextual UI.

Once activated, it changes to `HERMES · CHECKING CONNECTION` and resolves to
one of:

- `SUPPORTED`
- `NO CONCERNS`
- `REVIEW SUGGESTION`

A concern contains an evidence-backed proposed diff. It never mutates the
user-authored destination bridge automatically. The researcher may inspect and
accept or dismiss the suggestion later.

Destination review is independent from origin reconciliation. A slow review
cannot delay or invalidate the drag.

## Detach outcomes

Outcomes vary because graph structure varies, not because the gesture changes
meaning.

For a chain `A--B--C`, removing both old relationships around `B` produces
three singletons.

For a triangle `A--B--C--A`, removing `B`'s two old relationships leaves
`A--C` connected.

Hermes chooses which of the dragged event's old relationships remain
semantically valid. Deterministic graph functions calculate the consequence of
each candidate plan before Hermes submits it.

## Hermes reconciliation harness

The existing `HermesRunner` is a streaming research agent with broad tool
access. Reconciliation should use a separate, constrained runner. It should not
receive terminal, file, browser, web-search, delegation, or source-mutation
tools for an ordinary drag.

The deterministic substrate does the mechanical reasoning:

- Capture an immutable graph snapshot and revision.
- Identify old incident relationships.
- Identify protected destination relationships.
- Compute degrees, alternate paths, articulation effects, connected components,
  and affected event sets.
- Enumerate or validate candidate actions.
- Simulate the component delta and local layout impact.
- Reject changes outside the job's scope.
- Commit one validated plan atomically.
- Record its inverse for undo.

Hermes supplies the residual semantic judgment using a small tool/API surface.

### Proposed tool surface

`get_reconciliation_context(transaction_id)`

Returns the dragged event, its old neighbours, relationship labels, relevant
summaries and sources, the drop destination, and graph revision. It excludes
unrelated Canvas content.

`simulate_origin_plan(transaction_id, actions)`

Accepts proposed keep/remove/amend actions and returns deterministic effects:
new components, isolated events, affected articulation points, protected-edge
violations, and local layout impact. It never writes.

`submit_origin_plan(transaction_id, actions, rationale)`

Submits a final structured proposal. The backend validates the proposal against
the original snapshot and current revision before committing.

`submit_destination_review(transaction_id, verdict, evidence, proposed_diff)`

Stores an advisory review. A proposed destination mutation remains pending
until explicitly approved by the researcher.

Hermes receives no general graph-write function. It can propose only through
these bounded calls.

### Plan constraints

The validator rejects a plan that:

- Touches the newly created destination bridge.
- Touches an event or relationship absent from the job snapshot.
- Deletes an event.
- Reuses a stale graph revision.
- Produces a self-loop or missing endpoint.
- Violates URL uniqueness.
- Omits a rationale for a semantic change.
- Repeats or contradicts an action for the same relationship.

The backend, not Hermes, owns these invariants.

## Persistence and API boundary

The existing backend supports event/bridge upsert during research and clearing
the graph. It lacks stable client mutations, bridge deletion, graph revisions,
manual placement, transactions spanning several mutations, undo history, and a
long-lived channel for reconciliation results.

The minimum new persistence concepts are:

- Stable relationship IDs.
- Relationship provenance (`agent` or `user`).
- Persistent event world positions and manual-pin state.
- Monotonic graph revision.
- Drag transaction with pre-drop snapshot.
- Reconciliation job state.
- Mutation log containing an inverse operation.
- Advisory destination review.

### Synchronous drop endpoint

`POST /graph/drag-transactions`

The request contains the dragged event, source and destination world
coordinates, optional exact target event, and expected graph revision.

The response contains the committed destination placement, any new protected
bridge, next graph revision, transaction ID, reconciliation job ID, and undo
token.

This endpoint performs no LLM work.

### Asynchronous result channel

The current graph SSE stream closes after loading or research completion.
Reconciliation therefore needs a dedicated transaction stream or a persistent
graph mutation stream.

Proposed event types:

- `drag.committed`
- `reconciliation.started`
- `reconciliation.resolved`
- `reconciliation.fallback`
- `connection_review.completed`
- `graph.undo`

Each event carries a transaction ID and graph revision. The client ignores a
stale result rather than applying it to newer state.

## Failure and concurrency behavior

Hermes failure must not roll back the researcher's destination.

On timeout, malformed output, unavailable model, or stale revision:

- The dropped node and user-authored destination bridge remain committed.
- A deterministic fallback removes every pre-drop incident relationship in the
  reconciliation job's scope.
- The backend computes and commits the resulting component delta atomically.
- The same origin animation used for an all-remove Hermes plan makes the
  fallback visible.
- The mutation log records that fallback, so undo restores every removed
  relationship.

The rest of the Canvas remains usable. A second drag may run concurrently when
its event and old relationships do not overlap the unresolved transaction.
Overlapping operations wait or require the first transaction to be resolved;
they are never merged implicitly.

Undo while a job is pending cancels the job and applies the recorded inverse.
Undo after resolution reverses both the immediate destination mutation and the
committed Hermes plan.

## Split, promotion, and merge

### Artifact promotion

Dragging an expanded artifact into empty space immediately creates a pinned
draft event at the destination. The parent displays the same origin
reconciliation treatment.

Dragging the event hub itself carries the event and all currently displayed
artifact leaves as one visual unit. The leaves preserve their offsets from the
hub during the drag. Grabbing an individual leaf instead detaches that artifact
and begins promotion.

Promotion must be one backend transaction. The current store aliases a new
event to an existing event when the new primary URL matches one of the
existing event's artifacts. Creating the child before removing the artifact
would therefore collapse it back into its parent.

The draft can inherit the parent color and use the artifact label/source as
initial editable fields. Hermes may propose richer metadata afterwards.

### Event splitting

The first implementation should define splitting as one or more artifact
promotions. A claim-only split has no draggable subobject yet. Full-text
selection and excerpting can later make a claim or excerpt directly
detachable.

### Event merging

Creating a relationship and merging event identities are not the same drop
result.

A destructive identity merge should require a distinct central dwell target:
the destination node visibly begins to coalesce only after the cursor remains
over its core. A near-node magnetic drop creates a relationship; a sustained
core drop merges records.

For a merge, the target event remains canonical immediately. The source event
is retained as a recoverable revision/alias rather than deleted. Conflicting
title, date, summary, notes, artifacts, and relationships are reconciled after
the visual merge, with no source data discarded before an undo record exists.

This central merge target should not be implemented until ordinary event
detachment and artifact promotion have validated the gesture grammar.

## Testing methodology

Testing proceeds from deterministic graph behavior outward to LLM variance and
then human interaction.

### 1. Pure graph tests

Use small named graphs with exact expected deltas:

- Singleton.
- Leaf.
- Chain with an articulation point.
- Cycle with an alternate path.
- High-degree hub.
- Two dense communities connected by one sparse relationship.
- Several relationships with the same endpoints but different semantics.

Verify incident-edge scoping, component changes, alternate paths,
articulation effects, protected destination edges, plan simulation, and inverse
generation.

Promote selected cases from the saved 71-event graph into anonymized regression
fixtures. In particular, include `iran-us-framework-mou` as a degree-nine
articulation case and `cyber-pqc-production-2026` as a cross-community,
degree-six articulation case.

### 2. Backend transaction tests

Verify:

- A drop commits without waiting for Hermes.
- Expected-revision conflicts fail without partial writes.
- Destination relationships are protected from origin plans.
- Plans are atomic and idempotent.
- Malformed or out-of-scope plans change nothing.
- Process interruption leaves a recoverable pending job.
- Retry does not duplicate mutations.
- Undo restores events, relationships, positions, and revision state.
- Artifact promotion moves a URL atomically without triggering deduplication.
- Database migration preserves the existing 71 events and 85 bridges.

Run tests against temporary databases only.

### 3. Hermes contract evaluations

The agent evaluation corpus should contain fixed reconciliation contexts and
allowed-action envelopes. Evaluate properties rather than requiring one
identical semantic answer:

- Valid structured output.
- No action outside the supplied relationship set.
- No mutation of protected destination edges.
- A rationale tied to provided event/source content.
- A simulation call before destructive submission.
- Conservative behavior under insufficient evidence.

Run each ambiguous case repeatedly and across model upgrades. Record outcome
variance, latency, malformed-plan rate, and validator rejection rate. This
preserves useful nondeterminism while measuring whether the harness contains
it.

### 4. Flutter unit and widget tests

Verify:

- Pointer-down on an event cannot pan the camera after drag activation.
- Pointer-down on empty Canvas still pans.
- Click-versus-drag threshold is stable at every supported zoom.
- Screen/world coordinate conversion preserves the drop location.
- The node follows pointer events without awaiting a future.
- Escape and pointer cancellation restore the pre-drag state.
- The destination bridge appears on synchronous commit.
- Pending, resolved, fallback, and stale reconciliation events render
  correctly.
- A stale transaction result cannot move or mutate newer graph state.
- The dropped node remains pinned while origin nodes settle.
- Hover expansion and artifact clicks still work after cancelled drags.

Add golden tests for the origin afterimage and destination review box at 35%,
45%, 100%, and 280% zoom.

### 5. Performance tests

The pointer-move path performs no JSON encoding, database access, HTTP request,
layout regeneration, or LLM work.

Measure:

- Drag frame time against a 16.7 ms budget at 60 Hz.
- Local drop-commit latency separately from Hermes latency.
- Repaint bounds while the afterimage animates.
- Incremental settle cost on the 20-event largest component.
- Memory and painter-cache behavior during repeated drags.

The acceptance criterion is perceptual: delaying Hermes must not change drag
smoothness.

### 6. Hands-on acceptance protocol

Each manual test starts from a disposable copy of the saved SQLite snapshot.
The researcher performs one scenario at a time:

1. Drag a leaf into empty space.
2. Drag an articulation event into empty space.
3. Drag an event into another perceived community.
4. Cancel before drop.
5. Undo during reconciliation.
6. Let Hermes fail or time out.
7. Drag an artifact into a new event.
8. Drag an expanded event hub and verify that its artifact leaves travel with
   it.
9. Continue using unrelated Canvas regions while reconciliation runs.

For every scenario, observe the cursor coupling, destination immediacy, origin
legibility, eventual graph result, and undo fidelity. Implementation advances
only after the researcher approves the current scenario.

## Recommended implementation sequence

1. Extract pure graph-analysis and transaction types with tests.
2. Add stable bridge identity, graph revision, placement, mutation log, and
   SQLite migration.
3. Add synchronous drag transaction and undo endpoints.
4. Refactor Flutter pointer arbitration and implement local-only dragging.
5. Persist immediate destination placement/relationship without Hermes.
6. Add origin afterimage and mocked delayed reconciliation.
7. Add the constrained Hermes harness and validator.
8. Add advisory destination review.
9. Add atomic artifact promotion.
10. Consider central-dwell event merge only after the preceding gesture is
    proven.

This sequence validates the latency-sensitive UX with a deterministic fake
reconciler before introducing model behavior.

## Confirmed product decisions

- Reconciliation runs concurrently after the synchronous drop and cannot block
  the Canvas.
- The destination position remains pinned after reconciliation.
- Destination review is opt-in through a closable box and remains available
  later from contextual UI.
- Reconciliation failure deterministically removes all old relationships in
  scope; undo can restore them.
- The first implementation does not include multi-event selection or dragging
  a graph-theoretic subgraph.
- Dragging an event hub carries its displayed artifact leaves. Dragging one
  artifact leaf promotes that artifact.
