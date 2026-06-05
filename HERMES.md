# Hermes profile for AI News Canvas

This project uses a project-local Hermes home so it does not mutate or depend on
the user's normal `~/.hermes` agent.

Run:

```bash
scripts/setup-hermes-ainews
```

The setup script uses this repository's `.hermes` directory as the runtime
profile root and creates a profile named `ainews` with:

```bash
hermes profile create ainews --no-skills --no-alias
```

This means:

- the user's default Hermes profile under `~/.hermes` is not modified
- no global `ainews` shell alias is created
- the project profile starts with zero bundled skills
- project state, sessions, logs, and skills live under `.hermes/profiles/ainews`

The backend uses the actual profile directory as `HERMES_HOME` at runtime:

```text
HERMES_HOME=<repo>/.hermes/profiles/ainews
HERMES_PROFILE=ainews
```

That distinction matters. Pointing `HERMES_HOME` at `<repo>/.hermes` exposes the
root/default bundled skill catalog; pointing it at
`<repo>/.hermes/profiles/ainews` keeps this app profile lean. Verify with:

```bash
HERMES_HOME="$PWD/.hermes/profiles/ainews" HERMES_PROFILE=ainews hermes skills list
```

The expected result is `0 hub-installed, 0 builtin, 0 local`. Do not install the
generic bundled skill catalog into this profile.

The reusable profile context is tracked under:

```text
hermes/ainews/SOUL.md
hermes/ainews/memories/USER.md
```

`scripts/setup-hermes-ainews` copies those files into
`.hermes/profiles/ainews/` on each run. Commit changes to the tracked templates,
not to `.hermes/`, because `.hermes/` also contains auth, logs, sessions,
caches, and state databases.

For a narrow AI-news research agent, use Hermes tools/providers such as Exa,
SearXNG, Browse.sh/browser automation, STT, and Kokoro TTS through profile
configuration and profile context files.

SearXNG adds value as a broad discovery vector, not as an extraction backend. It
returns titles, URLs, and snippets from a meta-search pass, which gives Hermes a
wider candidate URL frontier. Exa remains the semantic discovery and extraction
backend. Browse.sh or browser automation is reserved for dynamic,
JavaScript-heavy, workflow-like, or extraction-hostile pages. Browserbase cloud
is optional: local Browse.sh is enough for ordinary local browser automation,
while the cloud service mainly adds hosted browser infrastructure, remote
sessions/contexts, proxies, persistence, and managed browser execution when a
local browser is not enough.

The project profile's spoken-response path uses Hermes' command TTS provider
surface:

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

`scripts/kokoro-tts` uses `KOKORO_TTS_BASE_URL` when configured. Otherwise it
checks the local OpenAI-compatible server at `http://127.0.0.1:8890`, starts
`~/kokoro-tts/server.py` when available, and only falls back to a cold local
`kokoro` CLI such as `~/kokoro-tts/bin/kokoro` when the server path is not
available.

During research, Hermes can choose to speak a useful mid-task aside by emitting:

```text
AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"..."}}
```

The backend routes that note to Kokoro TTS and does not forward it to the graph
UI. `AI_NEWS_VOICE_NOTE_INTERVAL` and `AI_NEWS_VOICE_NOTE_LIMIT` only throttle
these agent-chosen notes; they do not force routine progress narration. Stale
voice notes are dropped after `AI_NEWS_VOICE_NOTE_MAX_AGE` seconds so mid-task
asides do not play after the research has already finished. Live voice notes are
also capped by `AI_NEWS_VOICE_NOTE_MAX_CHARS` so they stay as short asides rather
than becoming full spoken reports.
