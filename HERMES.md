# Hermes integration: present contract and limits

Hermes is Not News's sole research runtime, not its application runtime. Rust
opens, renders, edits, migrates, and recovers the canvas; validates proposals;
commits SQLite transactions; and owns cancellation, activity, and voice bounds.
Hermes owns inference and tool execution.

Every binary embeds these public profile seeds:

```text
hermes/ainews/config.yaml       → <hermes-root>/profiles/not-news/config.yaml
hermes/ainews/SOUL.md           → <hermes-root>/profiles/not-news/SOUL.md
hermes/ainews/memories/USER.md  → <hermes-root>/profiles/not-news/memories/USER.md
```

`<hermes-root>` is `~/.hermes` on Linux and `%LOCALAPPDATA%\hermes` on Windows.
On startup, Not News creates the `not-news` directory and writes each seed only
when its destination is absent. It neither reads, copies, activates, nor changes
`default`; an unrelated nonempty profile already named `not-news` is rejected.
Earlier app-private Not News state migrates intact. Subsequent launches do not
refresh existing SOUL, USER, metadata, or general configuration; they only add
missing Not News variable names to the terminal passthrough list in the owned
`config.yaml`. “Bundled profile” therefore means a first-write seed, not a
versioned Hermes distribution or update channel. No private profile state enters
Git or a release payload.

Connections starts Hermes's machine dashboard with `not-news` selected; stdout
and stderr are discarded, so successful process creation does not prove that the
dashboard stayed open. Research likewise selects the profile explicitly:

```sh
HERMES_HOME="$HOME/.hermes" hermes -p not-news skills list
HERMES_HOME="$HOME/.hermes" hermes -p not-news acp --check
```

The application exposes no backend selector and supplies no model override.
Hermes's configuration therefore governs ACP inference. Not News has no declared
Hermes version range and checks only whether a regular file named `hermes` can
be found on `PATH`; provider authentication and ACP compatibility are discovered
when research runs. Rust suppresses private thought chunks, presents bounded
tool-call activity, assembles complete typed proposals from streamed chunks,
and terminates the process group on cancellation, silence, deadline, or output
excess.

Connections separately owns required Exa and SearXNG inputs, optional
Browserbase cloud, and optional Groq transcription. Exa, Browserbase, and Groq
use distinct accounts under OS-vault service `not-news-canvas`; writing a key
replaces that account, and deletion removes it without confirmation. SearXNG is
a plaintext `settings.json` value that can be overwritten but not cleared in the
GUI. The GUI neither imports nor falls back to ambient credentials for those
services. Linux packages do not install a Secret Service provider; KWallet or
GNOME Keyring availability, setup, unlock, and relock remain desktop concerns,
and a vault request has no application-level timeout.

The research child does not inherit the parent environment wholesale. It keeps
`PATH`, locale, timezone, TLS-certificate, proxy, temporary-directory, and
Windows process variables; redirects HOME and platform data/config/cache roots
under the `not-news` profile; and injects the Connections-resolved Exa, SearXNG,
and optional Browserbase values. `HERMES_MAX_TURNS` is an explicit developer
override, while an ambient `HERMES_HOME` can choose the Hermes root before the
child is built. Kokoro configuration is a separate Rust voice path and remains
environment-driven.

This is configuration isolation, not confinement. ACP is launched with Hermes
YOLO mode and hook acceptance enabled; Hermes may expose terminal, filesystem,
browser, code-execution, delegation, and skill-management tools. A prompt tells
the agent to work only in its scratch directory, but no OS sandbox enforces that
instruction. The redirected home also does not prevent access through absolute
paths. Treat Hermes and its configured provider/tool set as trusted code with
the user's authority. Exa, SearXNG, and Browserbase values are available to that
process and its tools. The prompt forbids credential disclosure, but the current
redactor derives its secret list from the parent environment before deferred
vault loading; leaked vault values can therefore reach displayed and persisted
agent output.

## Research policy

The prompt requires a discovery triad: SearXNG broadens the URL/snippet frontier,
Exa finds semantic neighbors and extracts content, and Browse handles dynamic or
extraction-resistant pages. This is agent policy, not startup enforcement. The
app validates that Exa and SearXNG values exist and makes a bounded SearXNG JSON
request before launch; it does not validate the Exa key, Browse executable,
Browse skills, Browserbase key, Hermes provider, or useful search results. The
SearXNG route instructs Hermes to invoke `curl`. The release ships neither
Browse, skills, nor `curl`—the generated profile deliberately has no bundled
skills—so a stock installation cannot honestly claim the complete triad until
the operator supplies it.

The current prompt sends the question and a bounded digest of saved event IDs,
titles, dates, and primary URLs to Hermes's configured provider. Discovery may
send queries or pages to Exa, SearXNG, Browse, and Browserbase. Hermes and Not
News retain separate session/log history. The app does not provide a complete
erase command for profile state, vault entries, settings, and graph data, and
uninstallation intentionally preserves per-user files.

Hermes may propose a sparse `voice.note`; it cannot play audio. Rust first
sequences the note in research history, then bounds its age, count, and text,
calls the environment-configured Kokoro endpoint directly, kills playback
descendants on cancellation, and deletes its scratch WAV. A syntactically valid
endpoint plus a discovered local player is reported as configured before the
endpoint is contacted; audible output remains runtime evidence.
