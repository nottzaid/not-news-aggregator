# ADR 0005: Sequence research output with its graph mutation

- Status: accepted
- Date: 2026-07-14
- Governs: external-agent ingestion, event identity, session recovery, schema
  migration, failure visibility

## Problem

Hermes is an untrusted, failure-prone subprocess: it can repeat a line, skip a
line, emit malformed data, outlive the window, or die between a graph write and
progress delivery. The legacy stream parses and writes sequentially but leaves
no durable session cursor. After a crash, neither the application nor an agent
can prove which proposals committed; replay may duplicate knowledge or attach a
bridge to the wrong identity.

## Decision

Schema version 2 adds `research_sessions` and an append-only
`research_output_log`. Every parsed output receives a zero-based sequence within
its session. The store accepts only the next sequence; an identical retry returns
the existing log result, while different data at that sequence fails. Event or
bridge mutation, graph revision, output row, and session cursor share one
`BEGIN IMMEDIATE` transaction. A failed append therefore leaves no event, alias,
bridge, revision, or cursor fragment.

Only typed proposals cross the agent/store boundary. Event acceptance preserves
the proven legacy identity rule: a normalized primary URL matching any URL of an
existing event creates an alias instead of a duplicate. Artifact URLs remain
globally unique after fragment removal, trailing-slash removal, and case
normalization. Bridge endpoints resolve through aliases, must already exist and
remain distinct, and produce a deterministic key from ordered endpoints plus a
dash/whitespace-normalized, case-folded label. Rejection consumes no sequence.

Messages, protocol errors, voice notes, completion, and failure occupy the same
ordered log even when they do not mutate the graph. A clean completion or error
closes the session. On application startup, any still-running session becomes
`interrupted`; its prompt, accepted graph, cursor, and log survive so retry is an
explicit user action rather than guessed subprocess resurrection. The log stores
normalized typed payloads, not environment variables, provider credentials, or
arbitrary process state.

Migration from version 0 or 1 reserves the writer, validates the current graph,
creates and integrity-checks a SQLite online backup, then advances schema in one
transaction. An empty first launch creates version 2 directly without a fake
backup.

## Rejected alternatives

- Treating stdout delivery as commit acknowledgement loses the boundary at
  process death and makes repeated lines ambiguous.
- Hash-only deduplication cannot distinguish legitimate repeated messages or
  preserve causal order; a session-local sequence can.
- Buffering the whole answer until Hermes exits discards useful validated work
  on timeout and delays visible progress.
- Retrying missing bridge endpoints later without recording deferral makes edge
  order nondeterministic. The orchestrator may defer in memory, but the store
  either accepts a currently valid bridge atomically or changes nothing.
- Persisting credentials or Hermes/OpenCode workspace state inside the graph
  couples research recovery to a developer login and contaminates release data.

## Consequences and evidence required

Research can extend knowledge while placement history remains append-only; both
advance the same graph revision but keep separate operation logs because their
inverse and retry semantics differ. An exact repeated proposal is audit-visible
once and cannot increment revision twice. A later proposal carrying identical
content may be logged without a semantic revision when it changes no graph row.

Acceptance requires tests for ordered and conflicting retries, source-identity
aliasing, global artifact deduplication, alias-resolved bridges, missing/self
endpoint rejection, log-insertion rollback, terminal-session rejection,
interrupted-session recovery, and verified version-1 backup. Release evidence
must additionally kill the real subprocess during output and graph transactions,
then reopen the shipped database without knowledge loss or duplicated mutation.

Reverse this design only if another protocol proves atomic acknowledgement,
causal replay, idempotency, crash recovery, credential separation, and legacy
identity compatibility with a smaller persistent surface.
