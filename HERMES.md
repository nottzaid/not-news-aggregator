# The Hermes boundary

Hermes is Not News's sole research runtime, not its application runtime. Rust
opens, renders, edits, migrates, and recovers the canvas; validates proposals;
commits SQLite transactions; and owns cancellation, activity, and voice bounds.
Hermes owns inference and tool execution.

Every binary embeds and automatically installs this public profile:

```text
hermes/ainews/config.yaml       → <hermes-root>/profiles/not-news/config.yaml
hermes/ainews/SOUL.md           → <hermes-root>/profiles/not-news/SOUL.md
hermes/ainews/memories/USER.md  → <hermes-root>/profiles/not-news/memories/USER.md
```

`<hermes-root>` is `~/.hermes` on Linux and `%LOCALAPPDATA%\hermes` on Windows.
`not-news` is a sibling of the user's real `default`; installation neither
reads, copies, activates, nor changes `default`. An unrelated profile already
named `not-news` is rejected rather than appropriated. Earlier app-private Not
News state migrates intact; upgrades add missing policy but never replace
Hermes-owned authentication, provider choice, sessions, memory, logs, caches,
or databases. Private profile state enters neither Git nor release payloads.

Connections opens Hermes' machine dashboard with `not-news` selected. Research
likewise selects that profile explicitly:

```sh
HERMES_HOME="$HOME/.hermes" hermes -p not-news skills list
HERMES_HOME="$HOME/.hermes" hermes -p not-news acp --check
```

The application exposes no backend selector and supplies no model override.
Hermes' configured provider therefore governs ACP. Rust suppresses private
thought chunks, presents bounded tool-call activity, assembles complete typed
proposals from streamed chunks, and terminates the process group on
cancellation, silence, deadline, or output excess.

Connections separately owns Not News's fixed inputs: required Exa and SearXNG,
optional Browserbase cloud, and optional Groq transcription. Secrets reside in
the OS vault; the SearXNG URL resides in application settings. The research
child begins with an empty environment, a profile-owned home, minimal
execution/locale/TLS/proxy plumbing, and only those explicitly resolved values.
`PATH` may locate tools such as Browse; no ambient credential file or ordinary
home directory is searched.

## Research policy

SearXNG broadens the URL and snippet frontier; Exa finds semantic neighbors and
extracts fuller content; Browse.sh handles dynamic or extraction-resistant
pages. The policy treats them as complementary evidence paths and prefers
releases, documentation, papers, filings, standards, and original
announcements. Secondary reporting may reveal leads but cannot replace
available primary evidence.

Hermes may propose a sparse `voice.note`; it cannot play audio. Rust first
sequences the note in research history, then bounds its age, count, and text,
calls configured Kokoro directly, kills playback descendants on cancellation,
and deletes every scratch WAV.
