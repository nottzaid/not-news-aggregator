# Not News

Not News turns a question into a durable, spatially unbounded graph of findings,
source artifacts, and explicit relationships. Dragging changes placement, never
meaning; relating, detaching, and promoting sources are deliberate commands in
the same restart-safe undo history as movement.

One native Rust process owns the product: winit handles desktop input and
lifecycle, Skia reproduces the reference composition, and bundled SQLite owns
the graph, migrations, verified backups, research activity, and reversible
mutation. Hermes is the sole research runtime, but never the canvas runtime: a
saved graph remains open and editable without Hermes, network access, or an
inference provider.

Download the current unsigned Windows/Linux build from
[Not News 0.2.1](https://github.com/muradkant/not-news-aggregator/releases/tag/rust-v0.2.1).

## Operate the canvas

- `Ctrl+K` or `/` opens the text composer. The record orb captures a spoken
  question when Groq transcription is configured.
- `Ctrl+,` opens Connections. Hermes provider/model credentials stay inside
  Hermes; Not News separately collects Exa, SearXNG, optional Browserbase cloud,
  and optional Groq transcription configuration.
- Scroll forward to zoom in; drag empty space to pan; drag a finding to move it.
  The world has no fixed bounds, and none of these gestures creates a fact.
- Right-click a finding or source, or hover and press `Ctrl+E`, to relate,
  detach, or promote it explicitly.
- `Ctrl+Z` undoes; `Ctrl+Shift+Z` or `Ctrl+Y` redoes the latest committed move
  or semantic command, including after restart.
- Drop a legacy `graph.sqlite` onto a pristine canvas to import it. The source
  is read-only and is neither migrated nor overwritten.

The database defaults to `$XDG_DATA_HOME/not-news-canvas/graph.sqlite` on Linux
(`~/.local/share` fallback) and
`%LOCALAPPDATA%\not-news-canvas\graph.sqlite` on Windows. Explicit paths exist
for controlled recovery and testing:

```sh
not-news-app --database /path/to/graph.sqlite
not-news-app --database /path/to/new.sqlite --import-legacy /path/to/old.sqlite
```

## Configure research

The release does not install Hermes, Browse, SearXNG, or a model provider. A
saved canvas needs none of them. The implemented launch gate for new research
requires:

- [Hermes](https://hermes-agent.nousresearch.com) on `PATH`, with a provider and
  model configured through Hermes;
- an Exa key entered through Connections and stored under Not News's own account
  in Windows Credential Manager or Linux Secret Service;
- a SearXNG base URL entered through Connections and stored in plaintext
  application settings.

The tracked research policy additionally requires Browse CLI for dynamic or
extraction-resistant pages and invokes `curl` for SearXNG queries. The launch
gate enforces neither executable, and this release installs neither Browse,
Browse skills, nor `curl`.

Browserbase is an optional Browse execution surface. Groq is optional and serves
question transcription, not Hermes inference. Re-entering an Exa, Browserbase,
or Groq key replaces that Not News vault entry; the corresponding removal row
deletes it. Not News neither discovers nor copies credentials from other apps.
On Linux the release does not install a Secret Service provider: KWallet or
GNOME Keyring must already expose one, and first access may prompt for setup or
unlock. SearXNG has no equivalent removal row; a later valid URL overwrites it.

Spoken research notes are separate from Groq transcription. They use Kokoro
configuration inherited by the Rust application and a discovered local WAV
player; neither is configurable in Connections, bundled, or probed end-to-end
before the first note.

First launch creates missing files for Hermes profile `not-news` beside, without
reading or selecting, `default`. It does so even when Hermes is absent. Existing
profile policy is not generally upgraded: files already present remain intact,
although the owned `config.yaml` terminal passthrough list gains required Not
News variable names. Connections opens Hermes's dashboard with `not-news`
selected; the app does not install Hermes or configure its provider.

Research inherits process plumbing such as `PATH`, locale, TLS, and proxy
variables, then receives only the Exa, SearXNG, and optional Browserbase values
resolved by Not News. Redirected home directories prevent accidental reuse of
ordinary user configuration; they are not an operating-system sandbox. Hermes
runs with its ACP tool authority, including terminal and filesystem tools, while
a prompt asks it to remain inside the session scratch directory. Those injected
keys are visible to Hermes and its tools; the prompt forbids disclosure, but the
present output filter does not guarantee redaction of vault-loaded values. Read
[HERMES.md](HERMES.md) before treating that request as a security boundary.

## Install a release

Each release contains:

- Linux: desktop-integrated `.deb`, relocatable AppImage, and `tar.xz`;
- Windows: current-user NSIS installer and relocatable `.zip`.

No payload requires Rust, Flutter, Python, Clang, or a source checkout. Release
attachments include SHA-256 sums, source/build identity, dependency and license
inventories, and renderer results. CI executes each portable form, exercises
window presentation plus a disposable SQLite import/mutation/reopen sequence,
and checks package installation, reinstallation, removal, and survival of a
per-user marker. Those checks do not authenticate Hermes, query Exa or SearXNG,
exercise Browse, unlock an OS vault, hear audio, or validate Linux under Wayland;
the Linux window evidence runs under X11/Xvfb. The authenticated
Hermes-to-canvas and live-vault tests remain opt-in. Unsigned previews may
trigger Windows reputation warnings; a self-signed certificate would not
satisfy the stable-release signing gate.

## Build and verify

Source development requires Rust 1.95. Linux also needs ALSA, Fontconfig,
FreeType, OpenGL, and X11/Wayland development libraries.

```sh
git clone https://github.com/muradkant/not-news-aggregator.git
cd not-news-aggregator
./scripts/doctor
./scripts/dev

cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p not-news-platform --example hidden_smoke
NOT_NEWS_FORCE_SOFTWARE=1 cargo run -p not-news-platform --example hidden_smoke
./scripts/package-linux
./scripts/verify-linux-release
```

`scripts/dev` may load ignored `.env` controls for Hermes bounds and optional
Kokoro output; it does not source Not News credentials or start an obsolete
backend. `scripts/searxng` manages a local development SearXNG instance.

The evidence suite combines domain properties, SQLite rollback and process-death
boundaries, bounded HTTP and subprocess fixtures, persisted UI-to-database
traces, GPU/raster presentation, package lifecycle checks, and decoded-pixel
comparison against immutable Flutter specimens. The 71-event performance probe
warms the instrumented input-to-swap path, measures 600 frames with refresh
waiting removed, rejects p99 above 16.667 ms, and verifies that its imported
source remained byte-identical. It is a controlled renderer/input benchmark,
not evidence of network, provider, discovery, or hardware-audio latency.
Ordinary presentation retains its clocks, easing, and vertical synchronization.

The final Flutter/FastAPI state survives on `experimental-optimization` and its
historical tag. Rust releases contain neither runtime.
