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
- `Ctrl+,` opens Connections: choose Auto/OpenCode/Hermes research, store or
  remove the Groq key, and open Hermes' own provider dashboard.
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

## Install a release

Tagged Rust releases provide two payloads per supported OS:

- Linux: a desktop-integrated `.deb`, plus relocatable AppImage and `tar.xz`
  forms. The `.deb` installs the Not News launcher and icon; an AppImage runs
  after `chmod +x` without a repository or compiler.
- Windows: a current-user NSIS installer with Start Menu identity, plus a
  relocatable `.zip`. Neither form needs Rust, Flutter, Python, Clang, or a
  source checkout.

Every release carries SHA-256 sums, exact source/build-tool identity, and a
machine-readable dependency/license inventory. The release workflow executes
each portable payload, installs each native package, exercises import and
durable mutation through the packaged executable, then uninstalls while proving
that per-user research remains. Engineering previews are unsigned and may
trigger Windows reputation warnings; signing is a stable-release gate, not a
claim simulated with a self-signed certificate.

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

Connections (`Ctrl+,`) persists Auto, OpenCode, or Hermes without storing a
provider secret. `AI_NEWS_RESEARCH_BACKEND` remains a deployment override. Auto
prefers the authenticated standalone OpenCode CLI and otherwise uses Hermes.
The application supervises either as a bounded process group, accepts only
typed research events, validates every graph proposal, and commits accepted
mutations transactionally.

Hermes retains ownership of its provider/model/API-key/OAuth registry;
Connections opens Hermes' dashboard rather than copying or reducing it. Groq
transcription is different because Not News calls Groq directly: Connections
stores that key in Windows Credential Manager or Linux Secret Service. KWallet
may request creation of an application-owned encrypted wallet on first save.
`GROQ_API_KEY` overrides the vault for managed deployments; there is no
plaintext settings fallback.

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
./scripts/package-linux
./scripts/verify-linux-release
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
