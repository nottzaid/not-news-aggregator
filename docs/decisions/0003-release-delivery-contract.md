# ADR 0003: Ship native Rust artifacts and diagnose external capabilities

- Status: accepted
- Date: 2026-07-14
- Governing delivery: [issue #5](https://github.com/muradkant/not-news-aggregator/issues/5)

## Context

A source tree is not a deliverable. The researcher must not install a compiler,
language SDK, repository, or private development layout to use the product.
Conversely, pretending the independently operated Hermes/search/provider stack
is part of one binary would hide real credentials, network services, runtime
ownership, and failure modes.

Flutter/Python are historical implementation sources, not release inputs. The
`experimental-optimization` branch, existing tags, and existing releases remain
immutable evidence; new product artifacts originate only from the Rust line
after parity and migration gates pass.

Packaging experiments compared two credible mechanisms:

- [`cargo-packager`](https://github.com/crabnebula-dev/cargo-packager) emits
  Windows NSIS/MSI and Linux AppImage/deb/pacman bundles;
- [`dist`](https://github.com/axodotdev/cargo-dist) builds archives/installers,
  release manifests, checksums, and generated GitHub publication workflows
  across target runners.

Tool breadth is not evidence of clean installation, safe upgrade, desktop
integration, or dependency completeness. Native package experiments selected
`cargo-packager` 0.11.8: one declarative identity emits Debian/AppImage and
NSIS payloads, while repository-owned scripts retain control of portable
archives, hashes, build identity, dependency/license inventory, and lifecycle
verification. `dist` remains credible but would duplicate that proven control
surface here.

## Decision

Treat clean-machine installation, execution, upgrade, recovery, and removal as
product contracts developed with the application.

- Publish versioned Windows and Linux artifacts from immutable commits. Each OS
  receives one desktop-integrated installation path and one relocatable artifact
  for diagnosis or users who reject installation.
- Linux receives `.deb`, AppImage, and `tar.xz`; Windows receives current-user
  NSIS and `.zip`. All forms contain the same optimized executable for their
  target. `Packager.toml` owns product identity; `scripts/package-*` and
  `scripts/verify-*` own reproducible naming and executable evidence.
- Package the optimized Rust executable, fonts, icons, licenses, and every owned
  runtime asset. Skia and SQLite are build-time/static implementation details;
  release users never compile them. Flutter and Python are not packaged.
- Store immutable program files separately from per-user graph data, logs,
  cache, configuration, and credentials through platform-native directories.
  Never infer writable paths from the repository or executable directory.
- Launch the saved canvas without Hermes, SearXNG, Browse.sh, provider
  credentials, transcription, synthesis, or a development checkout. Features
  requiring an unavailable external capability remain disabled with a precise
  diagnosis and remediation path; absence never damages accepted graph data.
- Before schema change, make and verify a recoverable backup. Upgrade is atomic;
  failed migration reopens the last compatible data or explains why it cannot.
  Uninstallation preserves user research unless the user explicitly requests
  data deletion.
- Release automation builds on native Windows/Linux runners, records source
  commit and tool versions, emits hashes and a machine-readable dependency/
  license inventory, and attaches artifacts plus limitations to one GitHub
  Release. Signing is required for a stable release once protected credentials
  exist; unsigned engineering previews identify themselves as such.
- A release candidate is rejected unless disposable clean machines install or
  unpack it, launch it, open a fixture graph, persist a placement, restart,
  upgrade from the prior supported version, recover from an injected migration
  failure, and uninstall without deleting the graph. A developer-machine launch
  or successful archive extraction is insufficient.

The mechanism remains replaceable, but changing it now requires lifecycle
evidence stronger than the selected path, not a broader feature table.

## Alternatives rejected

### Publish only archives

An archive can prove relocation but does not own desktop entries, shortcuts,
upgrade identity, uninstall behavior, or dependency declaration.

### Publish only distribution-native packages

Debian, RPM, and pacman packages integrate well but no one format covers the
supported Linux population. A relocatable artifact keeps diagnosis and recovery
possible without multiplying repository infrastructure prematurely.

### Require `cargo install` or a repository checkout

Both transfer compiler, native-link, Skia-cache, source-availability, and host
configuration risk to the researcher. They remain developer workflows only.

### Bundle every research dependency invisibly

Hermes, search services, browser automation, and providers have independent
credentials, licensing, network, update, and operational boundaries. Hiding
them makes installation appear simple while making failure and security
uninspectable.

## Consequences

- Packaging metadata, icons, platform paths, migration rollback, capability
  diagnosis, and clean-machine checks enter implementation before visual parity
  is declared complete.
- Portable and installed forms must open the same data format without locating
  data relative to themselves.
- External integrations need a first-run capability report and testable degraded
  modes; “works on the development machine” cannot close delivery acceptance.
- Release signing introduces protected-secret governance and must never run for
  pull requests or untrusted revisions.

## Reversal conditions

Change formats or packaging tools when clean-machine evidence shows broader
compatibility, safer upgrades, or less maintenance. Reconsider an external
capability boundary only when its runtime and licensing can be owned, updated,
diagnosed, and secured more reliably inside the distribution. Never reverse the
separation of immutable program files from recoverable user research.
