# Not News preview operating guide

Not News is an unsigned x86-64 Windows/Linux preview. Each release has one
ordinary Linux ELF and one ordinary Windows executable—no installer, archive,
or adjacent runtime directory. Windows may show a reputation warning.
Linux browser downloads may need `chmod +x not-news-linux-x86_64` once. Release
presentation checks cover X11/Xvfb, not a real Wayland compositor.

The Linux ELF absorbs the application, assets, and substantial native
libraries. It retains glibc 2.31+ and runtime-selected X11/Wayland,
keyboard-layout, and graphics-driver interfaces as host boundaries. ALSA is
linked into the executable, though host audio configuration can select external
plugins. The binary neither mounts nor extracts a bundled filesystem.

The canvas opens, edits, imports, undoes, and reopens saved graphs without
Hermes, a provider, discovery services, or network access. Press `Ctrl+,` for
Connections; `Ctrl+K` starts research only after readiness passes. Ordinary
uninstall removes program files and preserves user research. Hovered finding
summaries grow to their content until bounded by the viewport; `Tab` focuses an
overflowing summary, arrow keys and Page Up/Down scroll it, and `Esc` or `Tab`
returns to the canvas. Wheel over the open Hermes activity drawer to move
between its newest and oldest retained entries.

## New-research prerequisites

Not News installs none of these:

- [Hermes](https://hermes-agent.nousresearch.com/docs/), independently installed
  on `PATH`; configure its model, provider, and authentication in Hermes. Not
  News creates/selects only profile `not-news`, never Hermes `default`.
- [Browse](https://browse.sh/) on `PATH`, including the skills needed by the
  pages researched. Browserbase is an optional cloud execution surface.
- `curl` on `PATH`; use the [official downloads](https://curl.se/download.html).
- an Exa key and SearXNG HTTP(S) base URL entered through Connections. Exa,
  optional Browserbase, and optional Groq transcription keys use Not News
  accounts in Windows Credential Manager or Linux Secret Service. SearXNG is a
  private-mode plaintext application setting. No plaintext key fallback exists.

Before creating a durable research session, Not News runs the exact selected
profile command `hermes -p not-news acp --check`, `browse --version`, `curl
--version`, bounded vault reads, and a bounded SearXNG JSON search-contract
request. This proves those command/configuration layers only: provider
authentication, Exa authorization/quota, useful results, Browse skills/browser
launch, Browserbase, and full streamed ACP behavior remain use-time evidence.
Hermes compatibility is cached only for the executable bytes, owned profile
configuration bytes, and Not News policy version. Linux Hermes 0.18.2 is the
observed point; no version corridor or native Windows research claim exists.

## State, provenance, and erasure

New Linux state uses directories mode `0700` and files mode `0600`. Not News
does not import Exa, SearXNG, Browserbase, or Groq values from the environment or
other applications. An absolute `HERMES_HOME` may deliberately select another
Hermes root; allowlisted proxy plumbing may carry operational credentials but is
not a discovery input. Hermes and its tools receive required Connections values;
exact, percent-, base64-, and hex-encoded echoes are redacted before
app-controlled display or SQLite persistence. This is defense in depth, not
confinement: trusted Hermes tools retain the user's process/filesystem/network
authority and can transform or exfiltrate received values.

Tool homes and platform config/data/cache variables are redirected below the
owned profile, with `terminal.home_mode: profile`. This prevents accidental
ordinary-home configuration reuse; absolute paths and OS authority remain
available. Profile installation is locked, staged, atomic, and marker-owned.
Existing owned files are preserved; policy v2 is recorded separately. An
unmarked `not-news` collision stops profile setup. Hermes `default` is neither
read nor changed.

Each connection opens a local action surface for configure/replace/remove;
removal is not a replacement with an empty value. Connections can delete each
Not News vault key and the SearXNG setting. “Complete erase”
requires typing `ERASE`, exclusive access against other Not News instances, and
confirmed deletion of all three vault accounts before filesystem deletion. It
then removes the graph and known migration backups, settings/scratch state, and
only an exactly marker-owned Hermes `not-news` profile; unrelated profiles and
`default` remain. Vault stores and filesystems are not transactionally coupled:
a failure reports partial or unconfirmed deletion. A five-second vault timeout
means the outcome is unknown because an OS vault operation may finish later.

## Optional voice paths

Groq transcribes questions and is configured in Connections; it never supplies
Hermes inference. Synthesized research notes use the separate environment-driven
Kokoro endpoint and a discovered local WAV player. Neither is bundled or proved
reachable/audible before use. Playback occurs only when Hermes emits an explicit
`voice.note`; the research contract requires one Hermes-selected consequential
milestone and permits one earlier distinct milestone. Ordinary research output
is silent. Voice failure does not affect research or saved graphs.
