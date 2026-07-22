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

Download the current unsigned Windows/Linux prerelease from
[Not News 0.3.1](https://github.com/muradkant/not-news-aggregator/releases/tag/rust-v0.3.1).

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
- Hovering a finding opens a content-sized summary. When its bounded reader
  shows `TAB FOCUS TO READ`, press `Tab` to freeze canvas interaction; use arrow
  keys, Page Up/Down, Home/End, then `Esc` or `Tab` to return.
- The Hermes activity drawer follows the newest retained entry. Wheel upward
  over the open drawer to recover older entries and downward to return to the
  latest; the thumb shows position within the retained history.
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

The release does not install Hermes, Browse, SearXNG, `curl`, or a model
provider. A saved canvas needs none of them. New research requires:

- [Hermes](https://hermes-agent.nousresearch.com/docs/) on `PATH`, with a
  provider and model configured through Hermes;
- [Browse](https://browse.sh/) and `curl` on `PATH`;
- an Exa key entered through Connections and stored under Not News's own account
  in Windows Credential Manager or Linux Secret Service;
- a SearXNG base URL entered through Connections and stored in private-mode
  plaintext application settings.

Before creating a durable research session, the app runs `hermes -p not-news
acp --check`, `browse --version`, `curl --version`, bounded vault resolution,
and bounded SearXNG `/config` and queryless JSON-format probes that dispatch
no search. This proves executable, profile routing, ACP installation,
configuration, SearXNG identity, enabled-engine, and JSON-format layers;
Hermes provider authentication, Exa authorization or quota, responsive upstream
engines, useful results, Browse skills/browser launch, Browserbase, and
streamed task behavior remain use-time evidence. Browse is required apparatus but invoked only when browser retrieval
is needed. Linux Hermes 0.18.2 is one observed compatible point, not a version
corridor; native Windows Hermes research remains unproved.

Browserbase is Browse's optional cloud surface. Groq is optional question
transcription, not Hermes inference. Each Connections row opens its own
configure/replace/remove actions; replacement writes the same Not News vault
account, while removal returns that capability to `NOT CONFIGURED`. Not News
neither discovers nor copies those inputs from the ambient environment or other
apps. On Linux the release does not install a
Secret Service provider: KWallet or GNOME Keyring must already expose one, and
first access may prompt for setup or unlock. Vault work has a five-second
application deadline; timeout means the result is unconfirmed because the OS
operation may finish later. No plaintext credential fallback exists.

Spoken research notes are separate from Groq transcription. They use Kokoro
configuration inherited by the Rust application and a discovered local WAV
player; neither is configurable in Connections, bundled, or probed end-to-end
before the first note. The provider-neutral prompt requires one `voice.note`
for the milestone Hermes judges most consequential and permits one earlier note
for a distinct major milestone; ordinary progress, findings, and completion
messages are not spoken.

First launch transactionally creates a marker-owned Hermes profile `not-news`
beside, without reading or selecting, `default`; Hermes may be absent. Locked,
same-directory staging prevents simultaneous launches from exposing a partial
profile. Existing owned files remain intact; policy v2 is recorded separately
and enforces profile-local terminal home at runtime. An unmarked `not-news`
collision is rejected. Connections requests Hermes's dashboard with the named
profile; delegated browser opening remains unconfirmed and a command fallback
is shown. The app neither installs Hermes nor configures inference.

Research inherits bounded process plumbing such as `PATH`, locale, TLS, and
proxy variables, then receives only the Exa, SearXNG, and optional Browserbase
values resolved through Connections. Home/config/data/cache roots are redirected
below the profile to prevent accidental ordinary-home credential reuse. This is
configuration provenance, not an OS sandbox: trusted Hermes tools retain user
authority and see injected values. Exact, percent-, base64-, and hex-encoded
echoes are redacted before app-controlled display and SQLite persistence;
transformed exfiltration remains possible. Read [HERMES.md](HERMES.md).

## Run a release

Each release uploads exactly two application payloads:

- `not-news-linux-x86_64`, one ordinary Linux ELF;
- `not-news-windows-x86_64.exe`, one ordinary Windows executable.

They are the programs themselves: there is no package manager, installer,
archive, extraction step, adjacent runtime directory, or language toolchain. On
Linux, make the downloaded file executable once with `chmod +x
not-news-linux-x86_64`, then run it. On Windows, run the `.exe`; unsigned previews
may trigger a reputation warning.

The bundled font notices remain accessible inside either standalone executable
with `--licenses`.

The Linux build follows the established single-ELF model used by native desktop
applications: fonts, rendering, database, TLS, audio, and other substantial
libraries are linked into the ELF, while glibc and runtime-selected host
display/graphics interfaces remain operating-system boundaries. It targets
x86-64 glibc 2.31 or newer and supports native Wayland or X11. It never mounts
or extracts an embedded filesystem.

CI relocates and directly executes each final file. Both the automatic and
software renderers must present a hidden window and complete a disposable
SQLite import/mutation/reopen sequence. The Linux release additionally fails
if ALSA, C++, FreeType, Fontconfig, PNG, Brotli, bzip2, or Expat leaks back into
its shared-library table, or if its glibc requirement exceeds 2.31. These checks
do not authenticate Hermes, query Exa or SearXNG, exercise Browse, unlock an OS
vault, hear audio, or validate Linux under a real Wayland compositor; the Linux
window evidence runs under X11/Xvfb. The authenticated Hermes-to-canvas and
live-vault tests remain opt-in.

Deleting the executable does not delete research. Connections exposes a
separately confirmed complete erase: after all other instances close and the
user types `ERASE`, it confirms deletion of all Not News vault accounts, then
removes application state, known graph migration backups, and only an exactly
marker-owned Hermes profile. Vault and filesystem stores cannot form one
transaction, so partial/unconfirmed failure is reported.

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
./scripts/prepare-linux-release
./scripts/package-linux
./scripts/verify-linux-release
```

`prepare-linux-release` checks out the pinned upstream ALSA revision beneath
ignored `target/`, verifies its exact commit, and builds the static archive used
by the final ELF. The release host must also provide the static development
archives named by `package-linux`; the Ubuntu 20.04 release job installs and
tests that environment from scratch. The custom release linker changes only
those named substantial libraries to static linkage and leaves glibc plus host
hardware/display loading dynamic.

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
