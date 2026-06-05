from __future__ import annotations

import os
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[2]
PROJECT_ENV = PROJECT_ROOT / ".env"
HERMES_ENV = Path.home() / ".hermes" / ".env"

SAFE_ENV_KEYS = {
    "EXA_API_KEY",
    "GROQ_API_KEY",
    "SEARXNG_URL",
    "AI_NEWS_SEARXNG_URL",
    "AI_NEWS_SEARXNG_SEARCH_URL",
    "OPENCODE_GO_API_KEY",
    "GITHUB_TOKEN",
    "HF_TOKEN",
    "REGULATIONS_GOV_API_KEY",
    "CONGRESS_GOV_API_KEY",
    "LLM_PROVIDER",
    "LLM_BASE_URL",
    "LLM_API_KEY",
    "HERMES_PROFILE",
    "HERMES_HOME",
    "KOKORO_BASE_URL",
    "KOKORO_MODEL",
    "KOKORO_API_KEY",
    "KOKORO_TTS_BASE_URL",
    "KOKORO_TTS_BIN",
    "KOKORO_TTS_MODEL",
    "KOKORO_TTS_VOICE",
    "KOKORO_TTS_SPEED",
    "KOKORO_TTS_AUTOSTART_SERVER",
    "KOKORO_TTS_SERVER_START_TIMEOUT",
    "KOKORO_TTS_SERVER_HEALTH_TIMEOUT",
    "AI_NEWS_ENABLE_VOICE",
    "AI_NEWS_VOICE_TIMEOUT",
    "AI_NEWS_VOICE_SYNTH_TIMEOUT",
    "AI_NEWS_AUDIO_PLAYER",
    "AI_NEWS_AUDIO_PLAY_TIMEOUT",
    "AI_NEWS_VOICE_DEDUPE_SECONDS",
    "AI_NEWS_VOICE_NOTE_INTERVAL",
    "AI_NEWS_VOICE_NOTE_LIMIT",
    "AI_NEWS_VOICE_NOTE_MAX_AGE",
    "AI_NEWS_VOICE_NOTE_MAX_CHARS",
    "AI_NEWS_VOICE_SLOW_LOG_SECONDS",
    "HERMES_AGENT_ROOT",
    "STT_GROQ_MODEL",
    "GROQ_WHISPER_MODEL",
    "AI_NEWS_ENABLE_HERMES",
    "HERMES_MAX_TURNS",
    "ELEVENLABS_API_KEY",
    "VOICE_TOOLS_OPENAI_KEY",
    "OPENAI_API_KEY",
    "MINIMAX_API_KEY",
    "MISTRAL_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
}


def load_private_env() -> None:
    _load_env_file(PROJECT_ENV, override=False)
    _load_env_file(HERMES_ENV, override=False, only_keys=SAFE_ENV_KEYS)
    os.environ.setdefault("STT_GROQ_MODEL", os.getenv("GROQ_WHISPER_MODEL", "whisper-large-v3-turbo"))
    os.environ.setdefault("GROQ_WHISPER_MODEL", os.getenv("STT_GROQ_MODEL", "whisper-large-v3-turbo"))
    os.environ.setdefault("HERMES_PROFILE", "ainews")
    os.environ.setdefault("HERMES_HOME", str(PROJECT_ROOT / ".hermes"))


def _load_env_file(
    path: Path,
    *,
    override: bool,
    only_keys: set[str] | None = None,
) -> None:
    if not path.exists():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        if only_keys is not None and key not in only_keys:
            continue
        if not override and key in os.environ:
            continue
        os.environ[key] = _clean_env_value(value)


def _clean_env_value(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value
