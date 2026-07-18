# Hermes integration: enforced contract and limits

Hermes is Not News's independently installed research runtime, never its canvas
runtime. Rust opens, renders, edits, migrates, and recovers the graph without
Hermes or network access. Hermes alone owns its unrestricted provider registry,
model, authentication, inference, and tool execution; Not News exposes no
competing backend selector or model override.

## Owned profile

Each binary embeds public seeds for profile `not-news`:

```text
hermes/ainews/config.yaml
hermes/ainews/SOUL.md
hermes/ainews/memories/USER.md
```

The Hermes root defaults to `~/.hermes` on Linux and `%LOCALAPPDATA%\hermes` on
Windows; an absolute ambient `HERMES_HOME` deliberately selects another root.
First launch creates/selects `profiles/not-news` even when Hermes is absent.
Installation holds `<root>/profiles/.not-news.install.lock`, writes an exact
ownership marker before other stage contents, completes an interrupted owned
stage, syncs it, and atomically renames it into place. A destination or stage
without a recognized marker is rejected, not appropriated. An earlier
app-private owned profile migrates intact.

Existing seeded configuration, identity, memory, authentication, sessions, and
logs are never overwritten. Policy v2 is an atomic marker declaring
`terminal.home_mode=profile`; runtime also sets `TERMINAL_HOME_MODE=profile`, so
old customized owned configuration receives the provenance rule without a
rewrite. New Linux profile directories/files use `0700`/`0600`. The ordinary
Hermes `default`, sticky selection, and unrelated profiles are neither read,
copied, selected, nor changed.

## Compatibility and readiness

Every research request first resolves an executable file with platform-appropriate
executability and runs the exact consumed selection/self-check boundary:

```sh
HERMES_HOME=<root> TERMINAL_HOME_MODE=profile \
  hermes -p not-news acp --check
```

The check uses a cleared, profile-isolated environment and an eight-second
deadline. Success is cached process-locally only by SHA-256 of the executable
bytes, owned `config.yaml` bytes, and policy version. It establishes executable,
global/profile CLI syntax, profile routing, ACP installation, and Hermes's
self-check result. It does not prove provider authentication, available model,
tool semantics, initialization/update shapes used during a later session, or a
successful research turn. A failed or unknown peer blocks only new research
before a durable session exists; the canvas remains usable.

Linux Hermes 0.18.2 is the sole real compatibility datum. It is neither a
supported floor nor corridor. Deterministic incompatible peers verify fail-fast
behavior, but no native Windows Hermes research path or scheduled newest-upstream
matrix is yet proved. Independently updating Hermes can therefore expose drift;
the preflight narrows but does not eliminate that risk.

Connections requests `hermes -p not-news dashboard`. Successful spawn cannot
prove a delegated browser opened or the dashboard stayed ready, so the GUI says
“requested/unconfirmed” and supplies the exact terminal command. Not News never
invokes `profile use` and never configures a provider.

## Discovery inputs and secret handling

Connections separately owns required Exa and SearXNG inputs, optional
Browserbase cloud, and optional Groq transcription. Exa, Browserbase, and Groq
use separate accounts under OS-vault service `not-news-canvas`; SearXNG is a
private-mode plaintext `settings.json` field. The GUI can replace and remove
each value. Not News never resolves these inputs from ambient variables or
other applications. Linux requires an existing Secret Service-compatible
KWallet or GNOME Keyring; no plaintext fallback exists. Each vault request has
a five-second deadline. Because the OS worker cannot be forcibly cancelled, a
timeout reports an unconfirmed outcome and a late completion remains possible.

The research child inherits an allowlist of process plumbing—`PATH`, locale,
timezone, TLS certificates, proxies, temporary-directory, and required Windows
process variables—then receives Connections-resolved Exa/SearXNG and optional
Browserbase values. Proxy URLs may themselves carry operational credentials;
they are not discovery inputs, and their values join the output filter. The
child does not inherit ambient service credentials. `HOME` and
platform config/data/cache roots point below the profile. This prevents
accidental ordinary-home configuration reuse; it neither hides the real account
from the OS nor blocks absolute paths.

Vault secrets are loaded before launch into an explicit filter. Complete ACP
message chunks, tool titles, plans, protocol failures, stderr diagnostics, and
line-protocol events are sanitized before parsing, display, or SQLite
persistence. Exact values plus anticipated percent, standard/URL-safe base64,
and lower/upper hex encodings are replaced; buffers are bounded and zeroized
where practical. This blocks the tested app-controlled sinks, not arbitrary
transformation by Hermes or a tool.

## Authority and discovery policy

Hermes runs ACP with YOLO mode and hook acceptance. It may expose terminal,
filesystem, browser, code-execution, delegation, and skill-management tools.
The prompt confines work semantically to a scratch directory; no OS sandbox
enforces it. Treat Hermes, its configured provider, and its tools as trusted
code with the user's filesystem/network authority. “Profile isolation” denotes
configuration provenance, not confidentiality or adversarial confinement.

The research prompt requires SearXNG for frontier expansion, Exa for semantic
neighbors/extraction, and Browse for dynamic or resistant pages. Before session
creation Not News checks `browse --version`, `curl --version`, Exa vault
presence, and a bounded SearXNG JSON `results`-array contract. Browse is required
apparatus but need not run when a task needs no browser retrieval. These probes
do not consume Exa quota and do not prove Exa authorization, nonempty/useful
SearXNG evidence, installed Browse site skills, browser launch, Browserbase
authorization, or live results; those failures surface during the task.

The question and a bounded saved-graph digest go to Hermes's configured
provider. Discovery may send queries/pages to Exa, SearXNG, Browse, and optional
Browserbase. Hermes and Not News retain separate histories. Ordinary uninstall
preserves both. Connections' confirmed complete erase serializes against other
Not News instances, deletes all three Not News vault accounts, then application
data/known graph backups and only a marker-owned `not-news` profile. Vault and
filesystem deletion cannot be atomic; partial or unconfirmed failure is stated.

Groq transcribes questions and never supplies inference. Sparse synthesized
research notes use the separate environment-configured Kokoro endpoint and a
discovered WAV player. Neither is bundled or proved reachable/audible before
use. Only an explicit `voice.note` from Hermes enters this path. The prompt
requires one note for the milestone Hermes judges most consequential and permits
one earlier note for a distinct major milestone; progress, findings, and
completion messages remain silent. Failure degrades voice, not research or saved
work.
