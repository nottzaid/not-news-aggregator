# ADR 0005: Sequence external research with graph mutation

- Status: accepted
- Date: 2026-07-14
- Amended: 2026-07-16 for Hermes ACP, sibling-profile isolation, and GUI-owned
  discovery configuration

## Problem

An external agent can repeat, fragment, corrupt, stall, or stop output between
delivery and graph commit. Without a durable session cursor, restart cannot
distinguish committed knowledge from an unacknowledged proposal; blind replay
can duplicate findings or attach a relationship to the wrong identity.

## Decision

Schema version 2 adds `research_sessions` and append-only
`research_output_log`. Each typed proposal receives a zero-based session
sequence. The store accepts only the next sequence, returns the original result
for an exact retry, and rejects different data at an occupied sequence. Proposal
mutation, graph revision, output row, and session cursor share one
`BEGIN IMMEDIATE`; rejection consumes neither sequence nor graph state.

Event identity follows the legacy rule: a normalized primary URL matching any
existing URL creates an alias rather than a duplicate. Artifact URLs are
globally canonicalized. Bridge endpoints resolve through aliases, must exist and
differ, and derive a deterministic identity from ordered endpoints and a
normalized label.

Hermes is the sole agent runtime. The binary installs its tracked policy as
`<hermes-root>/profiles/not-news`, beside and independent of `default`, and
selects it with `-p not-news`. It refuses an unrelated collision, never changes
Hermes' active profile, and preserves provider configuration, credentials,
memory, and sessions. Connections opens the same profile's dashboard.

The research child starts with an empty environment and profile-owned home. Its
allowlist contains executable discovery, locale/TLS/proxy plumbing, bounded
Hermes controls, and Not News inputs explicitly resolved from Connections.
Hermes retains its open inference registry; Not News requires its own Exa key
and reachable SearXNG JSON endpoint, optionally supplies Browserbase cloud, and
diagnoses local Browse availability. No shell credential or ordinary home
configuration is inherited.

Hermes ACP streams JSON-RPC. Rust suppresses private thought, records bounded
tool titles as activity, and assembles complete typed lines before parsing.
Each accepted event, bridge, message, or voice note crosses immediately; process
groups, byte ceilings, idle/total deadlines, and cancellation remain Rust-owned.
Terminal and non-mutating messages occupy the same ordered log. Startup marks
unfinished sessions `interrupted` and retains prompt, graph, cursor, and output;
retry remains explicit.

## Rejected alternatives and evidence

Stdout receipt is not commit acknowledgement. Hash deduplication loses causal
position. Whole-answer buffering hides progress and discards valid early work.
Unrecorded bridge deferral makes edge order nondeterministic. Persisting agent
credentials or arbitrary process state would couple graph recovery to a private
login.

Tests cover ordered/conflicting retries, alias identity, artifact
canonicalization, bridge validation, transaction rollback, terminal rejection,
interrupted recovery, verified schema backup, bounded ACP streaming, cancellation,
environment scrubbing, and live vault-to-Hermes-to-SQLite graph production.
Reverse only for a smaller protocol that proves the same causal replay,
atomicity, crash recovery, identity compatibility, and credential separation.
