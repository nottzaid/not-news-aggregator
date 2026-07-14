# Rust rewrite inventory

Status: observed contract at `3220d3a`; this is an input to implementation,
not a promise to preserve accidental architecture.

## Product surfaces

### Canvas

- Render events and labeled semantic bridges in an unbounded world viewed
  through a camera.
- Pan from empty canvas; zoom around the pointer by wheel or platform pan/zoom;
  zoom controls expose in, out, reset, and a numeric percentage. Current bounds
  are 35%–280%.
- Hovering an expandable event opens its source artifacts and displaces nearby
  display positions without changing base world positions. Leaving the
  protected hub/artifact paths collapses after a short delay.
- An event with one direct source opens externally; a multi-source event opens
  a metadata sheet and individually clickable artifacts.
- Newly streamed research is auto-focused until manual camera input cancels
  following.
- The visual language includes event color/glow, dashed curved bridges,
  metadata, source labels, research status, and Hermes activity. Exact pixels
  and animation behavior are compatibility requirements: the Flutter
  application is the oracle, except for explicitly recorded semantic changes.

### Input and mutation

- Text research exists in the backend contract, although the current visible
  primary entry is microphone-driven and the default prompt remains embedded
  in the Flutter screen.
- Microphone capture produces an audio file, Groq Whisper returns text, and the
  transcript starts a research session.
- The clear control deletes saved graph state when no session, recording,
  transcription, or clear operation is active.
- Current drag uses six screen pixels as click/drag arbitration, persists a
  world placement, exposes `Ctrl/Cmd+Z`, and may target another event. The Rust
  rewrite intentionally preserves click/drag arbitration and durable placement
  while replacing the semantic consequences described below.

### Research

- Hermes is an external subprocess with a repository-isolated `ainews` profile.
- Its line protocol uses `AI_NEWS_EVENT:` followed by JSON. Accepted types are
  `event.upsert`, `bridge.upsert`, `session.message`, `session.error`,
  `session.done`, and `voice.note`; ordinary prose becomes a session message.
- Event input normalization accepts integer or hex color, artifact objects with
  alternate text keys, and bare artifact URLs; durable output uses one canonical
  DTO.
- Event identity is deduplicated by normalized primary/source URL. Aliases map
  an incoming ID to its canonical event; bridges are rewritten through aliases;
  mutation rejects self-loops and missing endpoints, while snapshots hide
  persisted bridges and placements whose events are missing.
- The research policy requires SearXNG breadth, Exa semantic discovery and
  extraction, and Browse.sh for dynamic pages by default.
- Sparse, agent-selected voice notes are synthesized through the existing
  Kokoro boundary and never become graph nodes.

## Canonical payloads

```text
ResearchEvent
  id: nonempty string
  title: nonempty string
  date: nonempty display string
  color: u32 ARGB value
  summary: nonempty string
  sourceLabel: nonempty string
  artifacts: [SourceArtifact]
  url: optional string

SourceArtifact
  text: nonempty string
  source: nonempty string
  url: nonempty string

EventBridge
  from: existing canonical event ID
  to: existing canonical event ID, distinct from `from`
  label: normalized nonempty string
```

Legacy bridge identity is the ordered string
`<from>::<to>::<normalized-label-key>`. The current store does not canonicalize
endpoint order, and every persisted bridge in both reference databases lacks a
`provenance` field.

## SQLite evidence

Both ignored reference databases pass `PRAGMA integrity_check` and must remain
read-only during verification.

```text
backend/data/backups/pre-detach-design-20260703.sqlite
  sha256: a6e46496aa637d0f9758ed6a1e84250137c90699e98ebc57ff55c41009805fa2
  events: 71
  bridges: 85
  aliases: 9
  tables: events, bridges, event_aliases only

backend/data/graph.sqlite
  sha256: de3c93b6816366fa6e834ac534c5c07fe77c05f415bb3ac9bf93c52820f73a68
  events: 71
  bridges: 81
  aliases: 9
  placements: 2
  drag_transactions: 2, both status=fallback
  revision: 4
```

The live database therefore records two post-baseline drags whose fallback
plans removed four legacy relationships. Compatibility means reproducing this
state, not silently restoring the backup.

Current schema:

```sql
events(id TEXT PRIMARY KEY, payload TEXT NOT NULL)
bridges(id TEXT PRIMARY KEY, payload TEXT NOT NULL)
event_aliases(alias TEXT PRIMARY KEY, canonical_id TEXT NOT NULL)
placements(event_id TEXT PRIMARY KEY, x REAL, y REAL, pinned INTEGER)
graph_meta(key TEXT PRIMARY KEY, value TEXT)
drag_transactions(
  id TEXT PRIMARY KEY,
  status TEXT,
  base_revision INTEGER,
  committed_revision INTEGER,
  payload TEXT,
  plan TEXT
)
```

`events.payload`, `bridges.payload`, drag payloads, and plans are JSON text. All
71 current event payloads contain `artifacts` arrays and the key union
`artifacts,color,date,id,sourceLabel,summary,title,url`. Bridge key union is
`from,label,to`.

Rust schema version 1 adds per-event placement generations and immutable
move/undo/redo rows without altering these legacy tables. Version 2 adds durable
research sessions plus one causally ordered output log; a typed proposal, any
event/alias/bridge change, revision, log row, and session cursor commit together.
Both upgrades require a verified online backup when a prior graph exists.

## Intentional incompatibilities

- Ordinary `MoveNode` never creates, removes, amends, or asks Hermes to review
  a relationship.
- A missing or failed semantic agent never causes all-remove fallback.
- Relationship creation requires an explicit semantic command and predicate;
  proximity is preview evidence, not a committed meaning.
- Undo/redo operates on logged commands and conflict-aware inverses rather than
  deleting all currently incident edges and replaying an old snapshot.
- Durable background jobs recover after restart and publish typed deltas rather
  than requiring 240 ms whole-graph polling.
- Internal Rust components do not communicate through loopback HTTP/SSE. Any
  future network API is an intentional product boundary, not a desktop-process
  implementation detail.

## Runtime boundary inventory

The current developer launch requires Flutter, Python/uv, Hermes, curl, and a
Podman/Docker SearXNG runtime. The Rust target supports Windows and Linux,
removes Flutter, Python/uv, FastAPI, and the loopback backend from the
application boundary. Its bounded direct-exec research adapter supports either
the standalone `opencode` CLI or `hermes`; current external capabilities remain:

- an authenticated `opencode` CLI or `hermes` plus configured profile state;
- Podman/Docker plus SearXNG unless search is later embedded or remote;
- Browse.sh and provider credentials used by Hermes;
- Groq transcription HTTP API;
- optional Kokoro synthesis and an audio player;
- external browser/source opening.

Configuration names should remain compatible initially so the existing `.env`
continues to work. Secret values must never enter logs, fixtures, Git history,
check output, or agent prompts except where the provider requires them.

## Evidence gaps to close before parity claims

- Direct Skia has raster/temporal gates for the background, graph, expansion,
  fixed desktop chrome, active metadata, and status panel; responsive/narrow
  chrome, research activity, the new text composer, and voice states still need
  comparable visual contracts.
- Native pointer tests cover unbounded pan, anchored zoom, hover grace,
  activation, artifact paths, placement-only drag, source opening, and durable
  undo/redo. Real-window latency, invalidation-area, high-node-count, device-loss,
  and driver/backend traces remain absent.
- Research now crosses a process-group/Job-Object supervisor, typed parser,
  sequenced SQLite acceptance, and direct in-memory canvas updates. Remaining
  parity gaps are capability diagnosis/remediation, voice capture and
  transcription, and explicit destructive clear. The activity drawer and
  generated-cluster camera follow are now source-derived raster/temporal
  contracts; manual pan cancels automatic camera ownership.
- Crash-injection proves transaction rollback and interrupted-session recovery;
  release evidence must still kill real application/backend processes at each
  boundary and exercise the shipped recovery UI.
- Windows has no current executable reference or packaging check. The rewrite
  cross-compiles the renderer/platform boundary and compiles the full workspace
  on a native Windows CI runner, but must still verify native window/input
  behavior, Skia backend selection, external process/browser/audio adapters,
  clean installation, and visual parity on Windows; macOS, mobile, and web are
  explicitly outside product scope.
- No installer or relocatable artifact yet proves platform-native data paths,
  clean-machine launch, external-capability diagnosis, safe prior-version
  upgrade, migration rollback, research-preserving uninstall, hashes, dependency
  inventory, or signing. Flutter/Python are archival and must not enter new Rust
  release artifacts.
