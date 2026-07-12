# Project-isolated Hermes

Not News Aggregator creates an `ainews` profile inside this repository; it does
not mutate or depend on the user's ordinary `~/.hermes` agent.

```sh
scripts/setup-hermes-ainews
```

The setup command creates the profile without bundled skills or a global alias,
opts out of skill installation, writes the narrow runtime configuration, then
copies the tracked context:

```text
hermes/ainews/SOUL.md          → .hermes/profiles/ainews/SOUL.md
hermes/ainews/memories/USER.md → .hermes/profiles/ainews/memories/USER.md
```

The backend executes with:

```text
HERMES_HOME=<repo>/.hermes/profiles/ainews
HERMES_PROFILE=ainews
```

That first path is exact. Pointing `HERMES_HOME` at `<repo>/.hermes` exposes the
root profile's bundled catalogue instead of the lean application profile.

Verify isolation:

```sh
HERMES_HOME="$PWD/.hermes/profiles/ainews" \
HERMES_PROFILE=ainews \
hermes skills list
```

Expected: `0 hub-installed, 0 builtin, 0 local`. Commit edits to the templates,
never ignored `.hermes/`, which also contains credentials, sessions, logs,
caches, and databases.

## Research policy

The profile separates discovery by capability:

- SearXNG for a broad, configurable URL and snippet frontier;
- Exa for semantic discovery and extraction;
- Browse.sh for dynamic or extraction-resistant pages.

Official releases, documentation, papers, filings, standards, and original
announcements outrank summaries. Secondary reporting discovers leads; it does
not displace available primary evidence.

## Voice path

The generated profile config uses a command provider:

```yaml
tts:
  provider: kokoro
  providers:
    kokoro:
      type: command
      command: "scripts/kokoro-tts --input {input_path} --output {output_path} --voice {voice} --speed {speed}"
      output_format: wav
      voice: af_heart
```

`scripts/kokoro-tts` tries `KOKORO_TTS_BASE_URL`, then a healthy local server at
`127.0.0.1:8890`, then autostarts `~/kokoro-tts/server.py`, and finally falls
back to a local `kokoro` executable.

Hermes may request one short aside with:

```text
AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"..."}}
```

The backend speaks but does not forward that event to the Canvas. Interval,
count, age, and character limits suppress narration, stale notes, and full
spoken reports.

## Reconciliation boundary

Research sessions deliberately receive the configured search surfaces.
Drag reconciliation does not: it runs with `--toolsets clarify --ignore-rules`,
without `--yolo`, and sees only the transaction context. Deterministic backend
validation—not the model—owns relationship scope, destination protection, and
fallback.
