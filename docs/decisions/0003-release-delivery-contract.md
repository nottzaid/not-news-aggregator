# ADR 0003: Treat native delivery as an executable product contract

- Status: accepted
- Date: 2026-07-14
- Governing delivery: [issue #5](https://github.com/muradkant/not-news-aggregator/issues/5)

## Problem

A source tree is not a desktop deliverable. Requiring a compiler, language SDK,
repository, or private development layout transfers integration risk to the
researcher. Conversely, disguising independently operated search, inference,
and browser services as one binary would conceal credentials and failure
boundaries rather than remove them.

Packaging experiments selected `cargo-packager` 0.11.8 for native identity and
installers while repository scripts retain artifact naming, archives, hashes,
inventories, and lifecycle evidence. A broader generator is not preferable
unless it proves a stricter installed result.

## Decision

- Linux receives `.deb`, AppImage, and `tar.xz`; Windows receives a current-user
  NSIS installer and `.zip`. Every form contains the same optimized target
  executable, fonts, icon, and licenses.
- Program files are immutable and separate from per-user graph, configuration,
  credentials, logs, and cache. Uninstallation preserves research.
- Payloads launch a saved canvas without Hermes, discovery services, provider
  credentials, transcription, synthesis, a compiler, or a checkout. Unavailable
  external capabilities fail visibly and locally.
- Every build records its exact commit, Rust and packager identity, SHA-256
  sums, dependency/license inventory, and executed renderer results.
- Native Windows/Linux runners execute every portable form; install, run,
  uninstall, reinstall, and remove the native package; and prove user data
  survives. The packaged self-check imports a read-only legacy graph, commits
  move/undo/redo, reopens exact state, rejects future schema, rolls back a
  malformed import, and presents an empty native window.
- Stable signing requires protected credentials. Unsigned previews identify
  themselves honestly; a self-signed certificate is not release evidence.

Skia and SQLite are build-time/static implementation details. Flutter, Python,
Clang, and Rust are absent from the runtime contract. Hermes and the configured
research services remain explicit external capabilities.

## Consequences

Packaging metadata, platform paths, migration recovery, degraded capability
diagnosis, and clean-install checks evolve with the application. Archives remain
useful for relocation and diagnosis; native installers own desktop identity and
removal. A release attaches only artifacts that completed their platform's
workflow.

Change format or tooling only when native evidence proves broader compatibility,
safer lifecycle behavior, or lower maintenance. Never reverse the separation of
program files from recoverable user research.
