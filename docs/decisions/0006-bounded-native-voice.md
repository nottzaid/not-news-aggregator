# ADR 0006: Bound native voice from capture through playback

- Status: accepted
- Date: 2026-07-14

## Problem

Voice is useful only if device, disk, network, provider, synthesis, queue, and
player failure cannot block the UI, leak scratch data, or corrupt a question.
The former Flutter/Python chain divided those lifetimes and left Kokoro playback
unbounded.

## Decision

CPAL owns ALSA on Linux and WASAPI on Windows. The device callback converts
supported interleaved samples to mono `i16` and writes only to a two-second SPSC
queue; a worker writes a native-rate WAV. Overflow, device loss, unsupported
format, empty audio, and the 24 MiB ceiling are terminal rather than producing a
plausible truncated utterance.

A second tap submits; a 500 ms hold or Escape cancels. Finalized audio receives
a bounded 45-second Groq `whisper-large-v3-turbo` request whose key comes only
from the Not News OS-vault entry. Success, failure, cancellation, and exit all
delete the recording. No shell, plugin, Python process, or converter participates.

A typed `voice.note` first enters research history, then a two-slot worker may
synthesize it outside graph transactions. Each session permits two notes at
least 35 seconds apart. Text loses markup and URLs, stops at 110 Unicode
characters, and expires after 12 seconds queued. Duplicate, stale, disabled,
throttled, or excess notes consume no provider or player work.

The worker calls a configured OpenAI-compatible Kokoro endpoint, bounds the
response to 32 MiB, requires WAV, and owns one scratch file. Linux selects a
known native player; Windows uses `SoundPlayer`. Both run in killable process
groups. Missing synthesis or playback disables speech while research continues.

## Evidence

Tests cover channel ordering, overflow rejection, multipart framing, bounded
responses, transcript parsing, Unicode limits, throttling, in-flight HTTP
cancellation, descendant termination, and scratch cleanup. Native release
evidence executes Windows audio processes; physical device, permission, and
audible-output validation remain hardware acceptance rather than claims made by
headless CI.
