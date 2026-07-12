# Not News Aggregator

Not News Aggregator is a Linux research canvas, not a feed reader. Ask a
question by voice or text; a project-isolated Hermes agent searches through
SearXNG, Exa, and Browse.sh, then streams sourced events and relationships into
a persistent graph.

The graph remains a workspace after research ends. Related events join existing
regions; unrelated work opens elsewhere; source artifacts stay attached to the
claims they support. Dragging an event moves and pins it immediately while
Hermes reconciles only the relationships left at its origin.

## What runs

```text
Flutter Linux canvas
  ├─ GET  /graph/stream           saved events, bridges, placements, revision
  ├─ GET  /research/stream        Hermes research as SSE mutations
  ├─ POST /audio/transcribe       Groq Whisper
  └─ POST /graph/drag-transactions
            │
            ├─ immediate SQLite placement + optional user bridge
            └─ bounded Hermes origin reconciliation → deterministic fallback
```

The local FastAPI backend stores the graph in `backend/data/graph.sqlite`.
Kokoro can speak sparse, agent-chosen orientation notes; those notes never
become graph nodes.

## Requirements

- Flutter with Linux desktop support (Dart 3.4+)
- Python 3.12+ through [`uv`](https://docs.astral.sh/uv/)
- Podman or Docker for local SearXNG
- Hermes on `PATH`
- Browse.sh as `browse` for dynamic-page inspection
- optional local Kokoro for spoken notes

## From clone to canvas

```sh
git clone https://github.com/muradkant/not-news-aggregator.git
cd not-news-aggregator
cp .env.example .env
```

For live research with the default provider, set:

```sh
AI_NEWS_ENABLE_HERMES=1
OPENCODE_GO_API_KEY=...
EXA_API_KEY=...
GROQ_API_KEY=...       # required only for microphone transcription
```

SearXNG already defaults to `http://127.0.0.1:8889`. Check the complete local
contract, then start every component:

```sh
./scripts/doctor
./scripts/dev
```

`scripts/dev` prepares the isolated Hermes profile, starts SearXNG, starts the
backend, waits for `/health`, then launches Flutter. It stops the backend when
Flutter exits. The API listens on `http://127.0.0.1:8765` unless
`AI_NEWS_BACKEND_PORT` says otherwise.

With `AI_NEWS_ENABLE_HERMES=0`, saved graph browsing still works and a failed
drag reconciliation takes the deterministic fallback; no live research model
is called.

## Research surfaces

The three search paths have distinct jobs:

- **SearXNG** widens the URL frontier across engines, categories, languages,
  dates, and pages.
- **Exa** supplies semantic discovery and fuller extraction.
- **Browse.sh** handles JavaScript-heavy, interactive, or extraction-hostile
  pages.

Hermes compares those paths, prefers primary material, and uses secondary
sources as leads when direct evidence exists. Its reusable profile instructions
live in `hermes/ainews/`; runtime sessions, auth, logs, memory, and cache stay in
ignored `.hermes/` state. [Hermes profile](HERMES.md) explains that boundary.

Manage SearXNG independently with:

```sh
scripts/searxng start
scripts/searxng test
scripts/searxng stop
```

## Direct manipulation

Press an event and move more than six screen pixels to drag; empty-canvas input
still pans. During the gesture, the event follows local pointer state—no HTTP,
database, layout regeneration, or model work occurs.

Drop near another event to create a protected user relationship, or drop in
empty space to move alone. The backend atomically checks the graph revision,
persists the pinned world position, creates any destination bridge, and returns.
The UI shows the new state before origin reconciliation finishes.

Hermes receives only the dragged event's former relationships and may keep,
remove, or relabel each. Its process is isolated from project rules, restricted
to a non-mutating clarification toolset, bounded by time and turn count, and
killed on timeout. Malformed output, disabled Hermes, or process failure removes
the old relationships deterministically; the destination remains authoritative.

After a targeted drop, **LET HERMES CHECK THIS** requests a separate advisory
review. It cannot mutate the user-authored destination. The backend also exposes
transaction undo, though the current Canvas has no undo control yet. See
[Detach and reconciliation](docs/detach-reconciliation-design.md) for shipped
invariants and remaining work.

## Operate the graph

Clear all saved graph state with the trash control or:

```sh
curl -X DELETE http://127.0.0.1:8765/graph
```

Inspect the current snapshot:

```sh
curl http://127.0.0.1:8765/graph
```

Secrets, sessions, databases, generated SearXNG configuration, Flutter output,
and dependency caches are ignored. Tracked templates and lockfiles reconstruct
the application without publishing private research.

## Verify

```sh
flutter analyze
flutter test
UV_CACHE_DIR="$PWD/.uv-cache" uv run --project backend pytest backend/tests
flutter build linux --debug
```

The tests cover layout stability, SSE reduction, transcription failures,
durable placement and revision streaming, immediate drag commits, protected
destination bridges, fallback, undo, stale-drop rejection, Hermes JSON parsing,
and the reconciliation command's tool boundary.
