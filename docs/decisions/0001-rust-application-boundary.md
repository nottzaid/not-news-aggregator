# ADR 0001: Collapse the local product boundary into Rust

- Status: accepted; rendering refined by ADR 0002
- Date: 2026-07-14
- Governing research: [issue #1](https://github.com/muradkant/not-news-aggregator/issues/1)

## Problem

The Flutter application and Python/FastAPI service were one local product split
across two processes, two toolchains, duplicate transport types, loopback
HTTP/SSE, snapshot polling, and divided ownership of SQLite and Hermes. That
transport was deployment machinery, not a public API. Its drag path also mixed
placement with semantic revision, allowing a motor gesture to imply knowledge;
language choice alone could not repair that category error.

## Decision

One native Rust process owns the canvas. Seven dependency-directed crates keep
its internal boundaries explicit:

```text
app → domain, store, agent, audio, renderer, platform
store → domain        agent → domain        renderer → domain
```

- `domain` owns graph types, commands, inverses, and pure invariants.
- `store` owns bundled SQLite, migrations, verified backups, journals, and
  recovery.
- `agent` owns typed Hermes ACP and bounded child-process supervision.
- `audio` owns capture, transcription, synthesis, playback, and cleanup.
- `renderer` turns immutable scene state into deterministic Skia output.
- `platform` owns winit, native surfaces, clipboard, paths, and presentation.
- `app` owns interaction state and orchestrates the other boundaries through
  typed data, never loopback transport.

`MoveNode` changes placement only. Relations, detachment, and source promotion
are explicit domain commands. Agent, network, audio, and renderer failure cannot
mutate accepted graph state outside a store transaction.

Hermes, SearXNG, Exa, Browse.sh, Groq, Kokoro, and the system browser remain
external because they are independently operated capabilities. The application
diagnoses their absence; it does not counterfeit ownership by bundling private
credentials or developer state.

## Consequences

SQLite compatibility is an initial executable contract. Background work reports
typed outcomes and never borrows UI state. A future network API must justify an
independently operated client or service; it may wrap domain commands but will
not dictate the in-process model.

The Flutter/FastAPI implementation remains historical evidence on
`experimental-optimization`; reference databases and rasters carry its data and
visual contracts into the Rust line without packaging either runtime.

Reverse the single-process boundary only when a separately deployed client or
service becomes a product requirement. Reconsider Rust only if the compatibility
and interaction contracts become materially simpler under another implementation.
