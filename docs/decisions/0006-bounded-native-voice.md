# ADR 0006: Bound native voice at capture, synthesis, and playback

- Status: accepted
- Date: 2026-07-14
- Governs: microphone capture, transcription, agent-selected voice notes,
  Kokoro synthesis, playback, cancellation, credentials, temporary data

## Problem

The record orb is the primary low-friction research input, while sparse spoken
notes orient a researcher whose attention remains on the canvas. The legacy
path delegates capture to Flutter, transcription to Python, and output to an
unbounded asynchronous Kokoro/player chain. Replacing those parts without one
ownership model would make voice responsive only while devices, disks,
networks, queues, credentials, synthesis, and playback all behave.

## Decision

CPAL owns the default ALSA microphone on Linux and WASAPI microphone on Windows.
The device callback converts supported `f32`, `i16`, or `u16` interleaved input
to mono `i16` and writes only to a two-second lock-free SPSC queue. A dedicated
thread drains that queue into a WAV at the device's supported sample rate.
Queue overflow, device loss, unsupported format, empty audio, and the 24 MiB
allowance are distinct terminal failures; no truncated utterance is submitted.

Stopping transfers the finalized WAV to a 45-second background Groq request.
The request uses `whisper-large-v3-turbo` unless the compatible model environment
variable overrides it, bounds response/error bodies, and reads
`GROQ_API_KEY` without logging, persisting, prompting an agent with, or packaging
it. Linux TLS uses Rustls; Windows uses Schannel. The recording owns deletion:
success, provider failure, cancellation, worker failure, and application exit
all remove it. No shell, Flutter plugin, Python process, or external converter
participates.

A tap starts capture and a second tap submits it. A 500 ms hold or Escape
cancels. Recording, transcription, and research share Flutter's busy orb; an
empty canvas names both `Ctrl+K` and the orb so text remains discoverable when
voice credentials or hardware are absent.

Agent `voice.note` output is first sequenced in the research log, then offered
to a two-slot output worker outside every graph transaction. Each research
session admits at most two notes, at least 35 seconds apart. Text is cleaned of
URLs and markup, truncated on a Unicode word boundary to 110 characters, and
discarded after 12 seconds in queue. Duplicate, throttled, excess, stale, and
disabled notes consume no synthesis or playback work.

The worker sends bounded JSON directly to a configured OpenAI-compatible
Kokoro `/v1/audio/speech` endpoint, accepts at most 32 MiB, requires a WAV
signature, and writes only an operation-local audio file. Linux chooses
`ffplay`, `pw-play`, `aplay`, or `paplay`; Windows uses its PowerShell
`SoundPlayer` surface. Both run as killable process groups. Cancellation races
the HTTP future and kills playback descendants; success, invalid output,
timeout, staleness, cancellation, and application exit delete the WAV. Missing
Kokoro, player, or authorization becomes visible capability status while
research ingestion and graph commits continue.

## Rejected alternatives

- Writing WAV samples in the device callback lets filesystem latency corrupt
  capture timing.
- An unbounded channel converts transient disk delay into memory growth.
- Overwriting old queue samples yields syntactically valid but semantically
  incomplete questions.
- Forcing 16 kHz at stream construction rejects microphones whose supported
  native configuration differs; Groq already normalizes speech input.
- Shipping a credential or developer workspace makes installation appear
  functional by transferring account authority rather than product capability.
- Persisting synthesized audio or scratch prose turns ephemeral orientation
  into sensitive research state without improving recovery.
- Letting the agent invoke a player directly loses queue, age, deduplication,
  cancellation, and cleanup authority at the application boundary.

## Evidence required

Acceptance requires stereo-to-mono ordering, overflow rejection, real multipart
framing, bounded authenticated response, transcript parsing, pre-delivery WAV
deletion, loopback synthesis-before-playback ordering, throttle/Unicode bounds,
in-flight HTTP cancellation, descendant-player termination, scratch cleanup,
warning-denied Linux compilation, Windows execution, and Flutter-oracle busy-orb
residuals. The local native specimen must audibly exit through Kokoro and leave
no file. Release evidence still requires physical ALSA and WASAPI input,
permission denial, device removal, clean-machine missing-capability remediation,
and packaged Windows playback.
