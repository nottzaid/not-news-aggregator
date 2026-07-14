# Project-isolated Hermes

Hermes is an optional research backend, never an application runtime. The Rust
canvas opens, edits, migrates, and renders saved knowledge without it. When
selected, Hermes searches and emits typed proposals; Rust owns validation,
SQLite transactions, activity history, voice throttling, and cancellation.

Install the tracked profile into private repository-local state:

```sh
scripts/setup-hermes-ainews
```

The command creates `.hermes/profiles/ainews`, disables bundled skills, and
installs only:

```text
hermes/ainews/config.yaml       → .hermes/profiles/ainews/config.yaml
hermes/ainews/SOUL.md           → .hermes/profiles/ainews/SOUL.md
hermes/ainews/memories/USER.md  → .hermes/profiles/ainews/memories/USER.md
```

Templates are public policy; ignored `.hermes/` contains authentication,
sessions, memory, logs, caches, and databases. Never copy or commit it. Add the
provider credentials and select the provider/model inside Hermes itself, then
verify the same boundary the application invokes:

```sh
HERMES_HOME="$PWD/.hermes/profiles/ainews" hermes skills list
HERMES_HOME="$PWD/.hermes/profiles/ainews" \
  hermes --oneshot 'Return exactly HERMES_PROFILE_OK' \
  --provider opencode-go --model mimo-v2.5-pro
```

Expected skills: zero hub-installed, builtin, and local. Select the backend
with:

```sh
AI_NEWS_RESEARCH_BACKEND=hermes \
AI_NEWS_HERMES_HOME="$PWD/.hermes/profiles/ainews" \
cargo run --release -p not-news-app
```

`AI_NEWS_HERMES_HOME` has priority over inherited `HERMES_HOME`; a named
`HERMES_PROFILE` is consulted only when neither exact home is available. Rust
uses Hermes' one-shot surface without replacing its configured provider or
model. `HERMES_PROVIDER` and `HERMES_MODEL` remain explicit deployment
overrides, not application defaults. Rust bounds turns through
`HERMES_MAX_ITERATIONS`, parses only `AI_NEWS_EVENT` lines, and kills the entire
process group on cancellation, silence, timeout, or output excess. Packaged
users open Hermes' provider/model/API-key/OAuth dashboard from `Ctrl+,` →
Connections; Not News neither mirrors nor narrows that registry.

## Research policy

SearXNG broadens the URL/snippet frontier; Exa supplies semantic discovery and
extraction; Browse.sh handles dynamic or extraction-resistant pages. The
profile compares these paths and prefers releases, documentation, papers,
filings, standards, and original announcements. Secondary reporting discovers
leads but does not replace available primary evidence.

Hermes may propose a sparse `voice.note`; it cannot play audio. The Rust worker
sequences the note in research history, bounds age/count/text, calls Kokoro
directly, kills playback descendants on cancellation, and deletes every
scratch WAV.
