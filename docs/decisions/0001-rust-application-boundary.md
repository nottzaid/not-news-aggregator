# ADR 0001: Collapse the local application boundary in Rust

- Status: accepted; renderer and platform scope superseded by ADR 0002
- Date: 2026-07-14
- Governing research: [issue #1](https://github.com/muradkant/not-news-aggregator/issues/1)

## Context

The shipped Linux application divides one local product across a Flutter
process and a Python/FastAPI process. Graph loading and research mutations cross
SSE; drag commits cross HTTP; settlement is polled as repeated whole-graph
snapshots; the backend owns SQLite and Hermes subprocesses. This separation is
not a product API: both processes are launched together, communicate through
loopback, and stop together.

The drag implementation also conflates spatial placement with semantic edge
revision. That fault must be corrected independently of language choice.

The user selected a Rust rewrite while requiring preservation of all Flutter
history, tags, data, and intentionally retained behavior.

## Decision

Build a Linux-first Cargo workspace with four dependency-directed crates:

```text
app ───→ domain
 ├────→ store ───→ domain
 └────→ agent ───→ domain
```

- `domain` owns graph/spatial types, commands, deltas, inverses, and pure
  invariants; it depends on no UI, database, runtime, or agent crate.
- `store` owns SQLite compatibility, migrations, mutation history, durable
  jobs, and recovery.
- `agent` owns the external Hermes line protocol, bounded child lifecycle,
  proposal validation, and provider-facing edges.
- `app` owns the desktop shell, interaction state machine, render scene,
  audio/network adapters, and typed orchestration. Its original eframe/egui
  implementation clause is historical and superseded by ADR 0002.

The desktop process calls domain/store/agent interfaces directly. It does not
recreate the loopback HTTP/SSE boundary. Hermes, SearXNG, Browse.sh, model APIs,
Kokoro, and the system browser remain external because they represent actual
capabilities outside the application.

The initial decision used eframe 0.35 with its default wgpu renderer for the
first measurable slice; ADR 0002 replaced it after that slice failed the paint
fidelity requirement. Keep domain, store, and interaction semantics
renderer-independent. Use
rusqlite 0.40 with a bundled SQLite build so application behavior does not vary
with the host SQLite version; enable only features demanded by backup,
migration, hooks, or profiling evidence.

The default drag command is `MoveNode`; it writes placement only. Explicit
relation and detachment commands will be designed separately. Agent failure is
non-destructive.

## Alternatives rejected

### Optimize Flutter and retain FastAPI

This could repair pointer jank, and remains proof that Rust is not the semantic
fix. It retains two language toolchains, DTO duplication, loopback transport,
whole-snapshot settlement, and split ownership of one local state machine.

### Rust UI over the existing Python backend

This changes the renderer while adding a third migration boundary and preserves
the accidental transport architecture. It is useful only as a disposable
renderer experiment, not the target system.

### Begin directly with winit/wgpu

This maximizes render control before evidence shows eframe is inadequate and
would spend the first implementation phase rebuilding windowing, text, input,
accessibility, and panels. The crate boundaries allow this substitution later.

### Embed every external dependency

Hermes and SearXNG have independent runtime/tool/provider semantics. Absorbing
them would expand the rewrite beyond the application and obscure which boundary
actually became simpler.

## Consequences

ADR 0002 later replaced the provisional eframe choice with direct Skia and
expanded product verification from Linux to Windows and Linux. The process,
crate-direction, persistence, command, and external-capability decisions here
remain accepted.

- The Python backend and Flutter client remain runnable reference systems until
  migration evidence permits their removal from the Rust branch.
- SQLite compatibility becomes the first executable contract, not a late port.
- Async jobs communicate with the UI through typed channels/deltas and never
  borrow UI state.
- The provisional eframe clause was exercised and reversed by ADR 0002; it is
  not part of the current dependency graph.
- A future public/network API must be justified as a product boundary and may
  wrap domain commands; it will not shape the in-process model by default.
- Historical branches, commits, tags, and releases are immutable evidence and
  are not cleanup targets.

## Reversal conditions

ADR 0002 has satisfied the renderer-specific reversal condition while retaining
this ADR's process and dependency decisions. Reconsider the single-process
boundary only if an independently operated client or service becomes a product
requirement. Reconsider Rust only if the vertical slice cannot reproduce
stored-data compatibility and required interaction behavior at lower total
complexity than the reference system.
