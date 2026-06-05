from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shlex
import shutil
import tempfile
import threading
import time
from pathlib import Path
from typing import NamedTuple

from .agent_events import AgentOutput
from .config import PROJECT_ROOT
from .kokoro_status import KokoroStatusModel


logger = logging.getLogger(__name__)
PROJECT_HERMES_HOME = PROJECT_ROOT / ".hermes"
HERMES_PROFILE = "ainews"
PROJECT_HERMES_PROFILE_HOME = PROJECT_HERMES_HOME / "profiles" / HERMES_PROFILE
PROJECT_KOKORO_TTS = PROJECT_ROOT / "scripts" / "kokoro-tts"
_VOICE_THREAD_LOCK = threading.Lock()
_RECENT_UTTERANCES: dict[str, float] = {}


class VoiceRunMetrics(NamedTuple):
    skipped: bool
    synth_seconds: float
    play_seconds: float


class HermesVoiceNotifier:
    def __init__(self, status_model: KokoroStatusModel | None = None) -> None:
        self.status_model = status_model or KokoroStatusModel()
        self._lock = asyncio.Lock()

    async def announce_start(self, prompt: str) -> str | None:
        if not _voice_enabled():
            return None
        text = self.status_model.complete(
            [
                {
                    "role": "system",
                    "content": (
                        "You are Hermes speaking aloud before starting a research task. "
                        "Say one natural sentence. Do not mention implementation details, "
                        "providers, JSON, graphs, or the word 'sidebar'."
                    ),
                },
                {"role": "user", "content": f"Research request: {prompt}"},
            ],
            fallback="I’m starting the research now, and I’ll map the important findings as they come together.",
            temperature=0.55,
            max_tokens=70,
        )
        return await self.speak(text)

    async def announce_done(
        self,
        prompt: str,
        outputs: list[AgentOutput],
        *,
        saw_mutation: bool,
    ) -> str | None:
        if not _voice_enabled():
            return None
        briefing = _briefing_context(prompt, outputs, saw_mutation=saw_mutation)
        text = self.status_model.complete(
            [
                {
                    "role": "system",
                    "content": (
                        "You are Hermes giving a short spoken post-task briefing. "
                        "Sound natural and specific. Mention what was found, not every detail. "
                        "Use one to three short sentences. No markdown."
                    ),
                },
                {"role": "user", "content": briefing},
            ],
            fallback=_fallback_done_line(outputs, saw_mutation=saw_mutation),
            temperature=0.5,
            max_tokens=120,
        )
        return await self.speak(text)

    async def speak(
        self,
        text: str,
        *,
        max_age_seconds: float | None = None,
    ) -> str | None:
        text = _clean_spoken_text(text)
        if not text:
            return "Hermes voice skipped an empty utterance."
        queued_at = time.monotonic()
        async with self._lock:
            wait_seconds = time.monotonic() - queued_at
            if max_age_seconds is not None and wait_seconds > max_age_seconds:
                return None
            try:
                started_at = time.monotonic()
                metrics = await asyncio.to_thread(
                    _synthesize_and_play_with_kokoro,
                    text,
                    queued_at,
                    max_age_seconds,
                )
            except Exception as error:
                return f"Hermes voice failed: {error}"
        _log_slow_voice_if_needed(
            wait_seconds=wait_seconds,
            synth_seconds=metrics.synth_seconds,
            play_seconds=metrics.play_seconds,
            text=text,
            skipped=metrics.skipped,
        )
        if metrics.skipped:
            return None
        return None


def _voice_enabled() -> bool:
    return os.getenv("AI_NEWS_ENABLE_VOICE", "1").strip().lower() not in {
        "0",
        "false",
        "no",
        "off",
    }


def _briefing_context(
    prompt: str,
    outputs: list[AgentOutput],
    *,
    saw_mutation: bool,
) -> str:
    titles = []
    bridge_count = 0
    status_tail = []
    for output in outputs:
        if output.type == "event.upsert" and isinstance(output.data, dict):
            title = str(output.data.get("title") or "").strip()
            if title and title not in titles:
                titles.append(title)
        elif output.type == "bridge.upsert":
            bridge_count += 1
        elif output.type == "session.message" and isinstance(output.data, dict):
            message = str(output.data.get("message") or "").strip()
            if message:
                status_tail.append(message)

    return "\n".join(
        [
            f"Original request: {prompt}",
            f"Canvas changed: {'yes' if saw_mutation else 'no'}",
            f"Event nodes added or updated: {len(titles)}",
            f"Relationships added or updated: {bridge_count}",
            "Event titles: " + "; ".join(titles[:8]),
            "Recent progress: " + " | ".join(status_tail[-5:]),
        ]
    )


def _fallback_done_line(
    outputs: list[AgentOutput],
    *,
    saw_mutation: bool,
) -> str:
    titles = [
        str(output.data.get("title"))
        for output in outputs
        if output.type == "event.upsert"
        and isinstance(output.data, dict)
        and output.data.get("title")
    ]
    if not saw_mutation:
        return "I finished the research pass, but I didn’t find enough structured evidence to add new canvas nodes."
    if not titles:
        return "I finished the research pass and updated the canvas with the relationships I found."
    if len(titles) == 1:
        return f"I finished the research pass and added {titles[0]} to the canvas."
    return (
        "I finished the research pass and updated the canvas with "
        f"{len(titles)} findings, including {titles[0]} and {titles[1]}."
    )


def _clean_spoken_text(text: str) -> str:
    text = text[:4000]
    text = re.sub(r"```[\s\S]*?```", " ", text)
    text = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", text)
    text = re.sub(r"https?://\S+", "", text)
    text = re.sub(r"[*_`#>-]+", " ", text)
    return " ".join(text.split())


def _synthesize_and_play_with_kokoro(
    text: str,
    queued_at: float,
    max_age_seconds: float | None,
) -> VoiceRunMetrics:
    with _VOICE_THREAD_LOCK:
        if _is_stale_voice(queued_at, max_age_seconds):
            return VoiceRunMetrics(skipped=True, synth_seconds=0, play_seconds=0)
        if _was_recently_spoken(text):
            return VoiceRunMetrics(skipped=True, synth_seconds=0, play_seconds=0)
        synth_started_at = time.monotonic()
        audio_path = _synthesize_with_project_kokoro(text)
        synth_seconds = time.monotonic() - synth_started_at
        if _is_stale_voice(queued_at, max_age_seconds):
            _unlink_quietly(audio_path)
            return VoiceRunMetrics(
                skipped=True, synth_seconds=synth_seconds, play_seconds=0
            )
        play_started_at = time.monotonic()
        try:
            _play_audio_file(audio_path)
        finally:
            _unlink_quietly(audio_path)
        play_seconds = time.monotonic() - play_started_at
        _remember_spoken(text)
    return VoiceRunMetrics(
        skipped=False, synth_seconds=synth_seconds, play_seconds=play_seconds
    )


def _synthesize_with_project_kokoro(text: str) -> Path:
    if not PROJECT_KOKORO_TTS.exists():
        raise RuntimeError(f"Project Kokoro TTS bridge is missing: {PROJECT_KOKORO_TTS}")

    import subprocess

    out_dir = Path(tempfile.gettempdir()) / "ai_news_canvas_voice"
    out_dir.mkdir(parents=True, exist_ok=True)
    input_path = out_dir / f"tts_{time.time_ns()}.txt"
    output_path = out_dir / f"tts_{time.time_ns()}.wav"
    input_path.write_text(text, encoding="utf-8")

    command = [
        str(PROJECT_KOKORO_TTS),
        "--input",
        str(input_path),
        "--output",
        str(output_path),
        "--voice",
        os.getenv("KOKORO_TTS_VOICE", "af_heart"),
        "--speed",
        os.getenv("KOKORO_TTS_SPEED", "1.0"),
    ]
    model = os.getenv("KOKORO_TTS_MODEL", "").strip()
    if model:
        command.extend(["--model", model])

    try:
        result = subprocess.run(
            command,
            cwd=PROJECT_ROOT,
            env=os.environ.copy(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=float(os.getenv("AI_NEWS_VOICE_SYNTH_TIMEOUT", "45")),
            check=False,
        )
    finally:
        _unlink_quietly(input_path)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "Kokoro TTS failed").strip()
        _unlink_quietly(output_path)
        raise RuntimeError(detail)
    if not output_path.exists() or output_path.stat().st_size == 0:
        raise RuntimeError("Kokoro TTS produced no audio.")
    return output_path


def _play_audio_file(path: Path) -> None:
    import subprocess

    command = _player_command(path)
    if command is None:
        raise RuntimeError("Kokoro generated audio, but no local player is available.")
    try:
        subprocess.run(
            command,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=float(os.getenv("AI_NEWS_AUDIO_PLAY_TIMEOUT", "20")),
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("Local audio playback timed out.") from error


def _player_command(path: Path) -> list[str] | None:
    configured = os.getenv("AI_NEWS_AUDIO_PLAYER", "").strip()
    if configured:
        return [*shlex.split(configured), str(path)]
    if shutil.which("ffplay"):
        return ["ffplay", "-nodisp", "-autoexit", "-loglevel", "quiet", str(path)]
    if shutil.which("aplay"):
        return ["aplay", "-q", str(path)]
    if shutil.which("afplay"):
        return ["afplay", str(path)]
    return None


def _unlink_quietly(path: Path) -> None:
    try:
        path.unlink()
    except OSError:
        pass


def _is_stale_voice(
    queued_at: float,
    max_age_seconds: float | None,
) -> bool:
    return (
        max_age_seconds is not None
        and max_age_seconds > 0
        and time.monotonic() - queued_at > max_age_seconds
    )


def _hermes_agent_root() -> Path:
    configured = os.getenv("HERMES_AGENT_ROOT", "").strip()
    if configured:
        root = Path(configured).expanduser()
        if _looks_like_hermes_source(root):
            return root
        raise RuntimeError(f"HERMES_AGENT_ROOT does not look like Hermes source: {root}")

    executable = shutil.which("hermes")
    if executable:
        root = _source_root_from_executable(Path(executable))
        if root is not None:
            return root

    fallback = Path.home() / ".hermes" / "hermes-agent"
    if _looks_like_hermes_source(fallback):
        return fallback
    raise RuntimeError(
        "Could not find Hermes source for TTS playback. Set HERMES_AGENT_ROOT."
    )


def _source_root_from_executable(executable: Path) -> Path | None:
    candidates = [executable.resolve()]
    try:
        text = executable.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        text = ""
    for match in re.findall(r'["\']([^"\']*/hermes-agent/venv/bin/(?:python3?|hermes))["\']', text):
        candidates.append(Path(match))

    for candidate in candidates:
        parts = candidate.parts
        if "hermes-agent" not in parts:
            continue
        index = parts.index("hermes-agent")
        root = Path(*parts[: index + 1])
        if _looks_like_hermes_source(root):
            return root
    return None


def _hermes_python(root: Path) -> Path:
    for name in ("python", "python3"):
        python = root / "venv" / "bin" / name
        if python.exists():
            return python
    executable = shutil.which("python3")
    if executable:
        return Path(executable)
    raise RuntimeError("No Python executable available for Hermes TTS playback.")


def _looks_like_hermes_source(path: Path) -> bool:
    return (path / "tools" / "tts_tool.py").exists() and (
        path / "tools" / "voice_mode.py"
    ).exists()


def _was_recently_spoken(text: str, now: float | None = None) -> bool:
    now = time.monotonic() if now is None else now
    ttl = _voice_dedupe_seconds()
    if ttl <= 0:
        return False
    key = _utterance_key(text)
    for stored_key, spoken_at in list(_RECENT_UTTERANCES.items()):
        if now - spoken_at > ttl:
            _RECENT_UTTERANCES.pop(stored_key, None)
    return key in _RECENT_UTTERANCES


def _remember_spoken(text: str, now: float | None = None) -> None:
    now = time.monotonic() if now is None else now
    if _voice_dedupe_seconds() <= 0:
        return
    _RECENT_UTTERANCES[_utterance_key(text)] = now


def _utterance_key(text: str) -> str:
    return re.sub(r"\W+", " ", text.lower()).strip()


def _voice_dedupe_seconds() -> float:
    try:
        return max(0.0, float(os.getenv("AI_NEWS_VOICE_DEDUPE_SECONDS", "20")))
    except ValueError:
        return 20.0


def _log_slow_voice_if_needed(
    *,
    wait_seconds: float,
    synth_seconds: float,
    play_seconds: float,
    text: str,
    skipped: bool,
) -> None:
    threshold = _voice_slow_log_seconds()
    if skipped or wait_seconds + synth_seconds + play_seconds < threshold:
        return
    logger.warning(
        "Hermes voice was slow: queued %.1fs, synth %.1fs, playback %.1fs, chars=%d",
        wait_seconds,
        synth_seconds,
        play_seconds,
        len(text),
    )


def _voice_slow_log_seconds() -> float:
    try:
        return max(0.0, float(os.getenv("AI_NEWS_VOICE_SLOW_LOG_SECONDS", "12")))
    except ValueError:
        return 12.0
