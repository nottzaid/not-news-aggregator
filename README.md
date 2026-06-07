# Not News Aggregator

Not News Aggregator is a Linux Flutter canvas for agentic research. You speak a
question from the GUI, Hermes researches it with Exa, SearXNG, and Browse.sh,
then streams the result into a living graph of events, source artifacts, and
relationships. The backend also exposes a prompt-based research stream for
direct API use.

This is not a feed reader. The canvas is meant to grow from research sessions:
related events join existing clusters, unrelated topics form separate regions,
and source cards stay attached to the event they support.

## What It Does

- Records speech from the Linux GUI and transcribes it with Groq Whisper.
- Runs a project-local Hermes profile so your normal Hermes setup is not
  modified.
- Uses Exa for semantic discovery and extraction.
- Uses local SearXNG as a broad meta-search discovery layer.
- Uses Browse.sh/browser automation for dynamic or extraction-hostile pages.
- Uses Kokoro for local spoken updates when Hermes decides a voice note is
  useful.
- Persists the graph in a local SQLite database.
- Provides a clear button in the bottom-right canvas controls.

## Requirements

- Flutter with Linux desktop support enabled.
- Python/uv for the FastAPI backend.
- Podman or Docker for local SearXNG.
- Hermes installed and available on `PATH`.
- Browse.sh CLI available as `browse` if you want dynamic-page inspection.
- Kokoro TTS installed locally if voice playback is enabled.

## Setup

Create your local environment file:

```bash
cp .env.example .env
```

Fill in the keys you use:

```bash
EXA_API_KEY=...
GROQ_API_KEY=...
OPENCODE_GO_API_KEY=...
AI_NEWS_ENABLE_HERMES=1
```

The default SearXNG settings are already suitable for local development:

```bash
SEARXNG_URL=http://127.0.0.1:8889
AI_NEWS_SEARXNG_SEARCH_URL=http://127.0.0.1:8889/search
```

## Run

Start everything with:

```bash
./scripts/dev
```

That script:

1. loads `.env`
2. prepares the project-local Hermes profile
3. starts local SearXNG
4. starts the FastAPI backend
5. launches the Flutter Linux GUI

The backend defaults to:

```text
http://127.0.0.1:8765
```

## Hermes Profile

This project uses:

```text
.hermes/profiles/ainews
```

The setup script creates it with no bundled skills and no global alias. This is
intentional: the app should not mutate your personal Hermes profile or install a
large generic skill catalog.

The project ships the profile context that makes the agent behave correctly:

```text
hermes/ainews/SOUL.md
hermes/ainews/memories/USER.md
```

`./scripts/dev` copies those templates into the ignored runtime profile under
`.hermes/profiles/ainews/`.

To verify the profile:

```bash
HERMES_HOME="$PWD/.hermes/profiles/ainews" HERMES_PROFILE=ainews hermes skills list
```

Expected result: no bundled or hub-installed skills.

## SearXNG

Local SearXNG is managed by:

```bash
scripts/searxng start
scripts/searxng test
scripts/searxng stop
```

The app asks Hermes to use SearXNG as a configurable discovery surface. Hermes
can choose categories, engines, time ranges, languages, and pages per task,
then compare that URL frontier with Exa semantic results.

## Clear The Canvas

In the GUI, use the trash icon in the bottom-right control strip.

From the terminal:

```bash
curl -X DELETE http://127.0.0.1:8765/graph
```

The graph database lives at:

```text
backend/data/graph.sqlite
```

It is ignored by git because it is local runtime state.

## Tests

Backend:

```bash
UV_CACHE_DIR="$PWD/.uv-cache" uv run --project backend pytest backend/tests
```

Flutter:

```bash
flutter test
```

Static analysis:

```bash
flutter analyze
```

Static analysis should pass cleanly. The Linux GUI is the primary target for
this prototype.

## Repository Hygiene

Ignored local state includes:

- `.env`
- `.hermes/`
- `.uv-cache/`
- `backend/data/`
- `build/`
- generated Flutter tool state
- generated SearXNG `settings.yml`

This keeps the open-source repository reproducible without publishing local
secrets, graph data, Hermes sessions, or generated build artifacts.
