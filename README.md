# Not News

Not News turns a question into a durable, spatially unbounded graph of findings,
exact sources, and explicit relationships. Dragging changes placement, never
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

First launch installs the bundled research policy as Hermes profile `not-news`,
beside an untouched `default`; the user launches no helper and chooses no
profile. Connections opens Hermes with `not-news` selected. Hermes alone owns
its providers, models, API keys, OAuth, sessions, memory, and logs.

Not News owns the research topology it can validate:

- Exa semantic discovery requires a key stored in Windows Credential Manager or
  Linux Secret Service.
- SearXNG breadth discovery requires a reachable JSON endpoint stored as
  non-secret application configuration.
- Browse.sh supplies browser automation and catalog skills; local use is
  keyless, while an optional Browserbase key enables its hosted capabilities.
- Groq speech-to-text is optional and uses the same OS vault.

The Connections surface is the sole source for those values. Shell credentials
and endpoints cannot silently override it. Each research child starts with a
profile-owned home and a scrubbed environment containing only minimal process
plumbing and explicitly resolved Not News inputs. Missing Exa or SearXNG blocks
new external research with a precise diagnosis; missing optional capabilities
does not endanger saved work. [HERMES.md](HERMES.md) specifies the boundary.

## Install a release

Each release contains:

- Linux: desktop-integrated `.deb`, relocatable AppImage, and `tar.xz`;
- Windows: current-user NSIS installer and relocatable `.zip`.

No payload requires Rust, Flutter, Python, Clang, or a source checkout. Each
also carries SHA-256 sums, exact source/build identity, dependency and license
inventory, and executed renderer evidence. Native workflows run every portable
form, install and reinstall each native package, exercise a real window and
durable mutation, then prove uninstallation preserves per-user research.
Unsigned previews may trigger Windows reputation warnings; a self-signed
certificate would not satisfy the stable-release signing gate.

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

The evidence suite combines domain properties, real SQLite rollback and process
death boundaries, bounded HTTP and subprocess tests, persisted UI-to-database
traces, GPU/raster presentation, package lifecycle checks, and decoded-pixel
comparison against immutable Flutter specimens. The 71-event performance probe
warms the exact input-to-swap path, measures 600 frames with refresh waiting
removed, rejects p99 above 16.667 ms, and verifies that its imported source
remained byte-identical. Ordinary presentation retains its clocks, easing, and
vertical synchronization.

The final Flutter/FastAPI state survives on `experimental-optimization` and its
historical tag. Rust releases contain neither runtime.
