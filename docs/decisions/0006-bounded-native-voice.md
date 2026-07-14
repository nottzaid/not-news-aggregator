# ADR 0006: Bound native voice before transcription

- Status: accepted
- Date: 2026-07-14
- Governs: microphone capture, audio ownership, transcription, cancellation,
  credentials, temporary data

## Problem

The record orb is the primary low-friction research input, yet the legacy path
delegates capture to Flutter and transcription to Python. Replacing either with
an unbounded callback, a shell helper, or a retained recording would make voice
responsive only while devices, disks, networks, and credentials behave.

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

## Evidence required

Acceptance requires stereo-to-mono ordering, overflow rejection, real multipart
framing, bounded authenticated response, transcript parsing, pre-delivery WAV
deletion, warning-denied Linux compilation, Windows-target audio compilation,
and Flutter-oracle busy-orb residuals. Release evidence still requires physical
ALSA and WASAPI devices, permission denial, device removal, network timeout, and
clean-machine credential remediation.
