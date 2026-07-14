# Not News Canvas

Not News is a native Windows/Linux research canvas. A question becomes a
durable graph of findings, exact sources, and explicit relationships; the
canvas remains spatially unbounded as years of work accumulate. Dragging moves
knowledge without inventing meaning. Relating, detaching, and promoting a
source are deliberate commands in the same append-only undo history as moves.

The application is one Rust process: winit owns native input and lifecycle,
direct Skia painting preserves the reference design, and bundled SQLite owns
graph state, migrations, verified backups, research activity, and reversible
mutations. There is no loopback web server or runtime Flutter/Python toolchain.
OpenCode or Hermes is an optional external researcher; a saved canvas opens and
remains editable without either.

## Use the canvas

- `Ctrl+K` or `/` opens the text composer; the record orb captures a spoken
  question when Groq transcription is configured.
- Drag a finding to organize space. Scroll forward zooms in; drag empty space
  to pan. Neither gesture changes graph semantics.
- Right-click a finding or source, or hover it and press `Ctrl+E`, to relate,
  detach, or promote it explicitly.
- `Ctrl+Z` undoes and `Ctrl+Shift+Z` or `Ctrl+Y` redoes the latest committed
  move or semantic command across restarts.
- Drop a legacy `graph.sqlite` onto a pristine canvas to import it read-only;
  the source file is never migrated or overwritten.

Program files and research are separated. By default the database lives at
`$XDG_DATA_HOME/not-news-canvas/graph.sqlite` on Linux (falling back to
`~/.local/share`) and `%LOCALAPPDATA%\not-news-canvas\graph.sqlite` on Windows.
For controlled recovery or testing:

```sh
not-news-app --database /path/to/graph.sqlite
not-news-app --database /path/to/new.sqlite --import-legacy /path/to/old.sqlite
```

## Build from source

Release artifacts are the user-facing delivery path. Development requires Rust
1.95; Linux linking additionally needs ALSA, Fontconfig, FreeType, OpenGL, and
X11/Wayland development libraries appropriate to the distribution.

```sh
git clone https://github.com/muradkant/not-news-aggregator.git
cd not-news-aggregator
./scripts/doctor
./scripts/dev
```

`scripts/dev` loads an optional ignored `.env` and runs the optimized binary.
It does not start an obsolete backend or require external research services.

## Optional research and voice

`AI_NEWS_RESEARCH_BACKEND=auto` prefers the authenticated standalone OpenCode
CLI and otherwise uses Hermes. Set it to `opencode` or `hermes` to make the
choice explicit. The application supervises either as a bounded process group,
accepts only typed research events, validates every graph proposal, and commits
accepted mutations transactionally. Credentials remain in the external tool's
private store.

The tracked `hermes/ainews` profile is installed into ignored project-local
state with `scripts/setup-hermes-ainews`; [HERMES.md](HERMES.md) defines its
isolation and search policy. SearXNG remains an optional independently managed
discovery surface through `scripts/searxng`.

Groq speech-to-text and OpenAI-compatible Kokoro speech output are direct,
bounded native adapters. Missing keys, devices, servers, or players disable the
affected capability without blocking research ingestion or graph access. Copy
`.env.example` only when configuring those optional surfaces.

## Evidence

```sh
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p not-news-platform --example hidden_smoke
NOT_NEWS_FORCE_SOFTWARE=1 cargo run -p not-news-platform --example hidden_smoke
```

The suite combines domain properties, real SQLite crash/rollback boundaries,
process cancellation, loopback HTTP framing, persisted UI-to-database traces,
GPU and software presentation, and decoded-pixel comparisons against immutable
reference rasters. Windows checks execute native process/audio tests and a real
hidden window. `fixtures/reference-raster` records visual and temporal
provenance; it is evidence, not a second implementation.

The prior Flutter/FastAPI application remains intact on
`experimental-optimization`, in tags, and throughout Git history. It is not
duplicated in this branch or included in Rust releases.
