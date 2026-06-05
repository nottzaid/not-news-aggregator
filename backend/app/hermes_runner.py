from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
import sqlite3
import time
from collections.abc import AsyncIterator
from pathlib import Path

from .agent_events import (
    EVENT_PREFIX,
    AgentOutput,
    normalize_agent_output,
    parse_agent_output,
)
from .config import PROJECT_ROOT
from .hermes_voice import HermesVoiceNotifier
from .kokoro_status import KokoroStatusModel
from .source_policy import SOURCE_POLICY_PROMPT


logger = logging.getLogger(__name__)
HERMES_PROFILE = "ainews"
PROJECT_HERMES_ROOT = PROJECT_ROOT / ".hermes"
PROJECT_HERMES_HOME = PROJECT_HERMES_ROOT / "profiles" / HERMES_PROFILE
HERMES_STATE_DB = PROJECT_HERMES_HOME / "state.db"


class HermesRunner:
    def __init__(
        self,
        status_model: KokoroStatusModel | None = None,
        voice_notifier: HermesVoiceNotifier | None = None,
    ) -> None:
        self.status_model = status_model or KokoroStatusModel()
        self.voice_notifier = voice_notifier or HermesVoiceNotifier(self.status_model)

    async def stream_research_updates(self, prompt: str) -> AsyncIterator[AgentOutput]:
        if os.getenv("AI_NEWS_ENABLE_HERMES", "0") != "1":
            yield AgentOutput(
                type="session.message",
                data={
                    "message": self.status_model.summarize(
                        "Hermes execution is disabled; streaming the fixture graph contract now."
                    )
                },
            )
            return

        env = self._hermes_env()
        command = self._hermes_command(prompt)
        _log_hermes_diagnostics(command, env)
        started_at = time.time()
        seen_outputs: set[str] = set()
        outputs: list[AgentOutput] = []
        voice_note_at = started_at - _voice_note_interval()
        voice_note_count = 0
        _schedule_voice(self.voice_notifier.announce_start(prompt))
        process = await asyncio.create_subprocess_exec(
            *command,
            cwd=PROJECT_ROOT,
            env=env,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )
        assert process.stdout is not None
        async for raw_line in process.stdout:
            line = raw_line.decode("utf-8", errors="replace").strip()
            line = _clean_hermes_status_line(line)
            if not line:
                continue
            _log_hermes_line(line)
            try:
                output = parse_agent_output(line)
            except (KeyError, ValueError, json.JSONDecodeError) as error:
                output = AgentOutput(
                    type="session.message",
                    data={"message": f"Ignored malformed Hermes graph event: {error}"},
                )
            try:
                output = normalize_agent_output(output)
            except (KeyError, ValueError, TypeError) as error:
                output = AgentOutput(
                    type="session.message",
                    data={"message": f"Ignored invalid Hermes graph payload: {error}"},
                )
            if output.type == "voice.note":
                _mark_seen_output(seen_outputs, output)
                if _should_schedule_voice_note(
                    output,
                    last_spoken_at=voice_note_at,
                    spoken_count=voice_note_count,
                ):
                    voice_note_at = time.time()
                    voice_note_count += 1
                    _schedule_voice(
                        self.voice_notifier.speak(
                            _voice_note_text(output),
                            max_age_seconds=_voice_note_max_age_seconds(),
                        )
                    )
                continue
            if output.type == "session.message":
                message = (
                    output.data.get("message", "")
                    if isinstance(output.data, dict)
                    else str(output.data)
                )
                output = AgentOutput(
                    type="session.message",
                    data={"message": self.status_model.summarize(message)},
                )
            _mark_seen_output(seen_outputs, output)
            logger.info("Hermes emitted %s", output.type)
            outputs.append(output)
            yield output
        code = await process.wait()
        for output in _harvest_state_graph_outputs(started_at, seen_outputs):
            if output.type == "voice.note":
                continue
            outputs.append(output)
            yield output
        if code != 0:
            output = AgentOutput(
                type="session.error",
                data={
                    "message": self.status_model.summarize(
                        f"Hermes exited with status {code}."
                    )
                },
            )
            outputs.append(output)
            yield output
        else:
            _schedule_voice(
                self.voice_notifier.announce_done(
                    prompt,
                    outputs,
                    saw_mutation=any(
                        output.type in {"event.upsert", "bridge.upsert"}
                        for output in outputs
                    ),
                )
            )

    def _hermes_env(self) -> dict[str, str]:
        env = os.environ.copy()
        env["HERMES_HOME"] = str(PROJECT_HERMES_HOME)
        env["HERMES_PROFILE"] = HERMES_PROFILE
        env.setdefault(
            "AI_NEWS_SEARXNG_URL",
            os.getenv("AI_NEWS_SEARXNG_URL")
            or os.getenv("SEARXNG_URL", ""),
        )
        env.setdefault(
            "AI_NEWS_SEARXNG_SEARCH_URL",
            os.getenv("AI_NEWS_SEARXNG_SEARCH_URL")
            or _searxng_search_url(env.get("AI_NEWS_SEARXNG_URL", "")),
        )
        env.setdefault("OPENCODE_GO_API_KEY", os.getenv("OPENCODE_GO_API_KEY", ""))
        return env

    def _hermes_command(self, prompt: str) -> list[str]:
        return [
            "hermes",
            "--profile",
            HERMES_PROFILE,
            "chat",
            "--query",
            self._research_prompt(prompt),
            "--provider",
            os.getenv("HERMES_PROVIDER", "opencode-go"),
            "--model",
            os.getenv("HERMES_MODEL", "mimo-v2.5-pro"),
            "--yolo",
            "--source",
            "ai-news-canvas",
            "--max-turns",
            os.getenv("HERMES_MAX_TURNS", "12"),
        ]

    def _research_prompt(self, prompt: str) -> str:
        return (
            "Research this AI-news question and report progress suitable for an event graph Canvas. "
            "Do not mutate the app source. Emit concise findings about events, source artifacts, "
            "and relationships. When you discover a graph mutation, print it on a single line as "
            '`AI_NEWS_EVENT: {"type":"event.upsert","data":{...}}` or '
            '`AI_NEWS_EVENT: {"type":"bridge.upsert","data":{...}}`. '
            "If, and only if, a spoken aside would genuinely help the user while you work, "
            "print one natural voice note on a single line as "
            '`AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"..."}}`. '
            "Use voice notes sparingly: only for meaningful orientation, a notable obstacle, "
            "or a useful mid-task finding; never narrate routine steps or low-value progress. "
            "Keep each voice note under 110 characters. "
            "Use the Flutter DTO keys exactly: id, title, date, color, summary, sourceLabel, "
            "artifacts, url for events; every artifact must use text, source, url; bridges use "
            "from, to, label. Bridge from/to values must exactly match event ids already in the "
            "Canvas or emitted in this same response; never invent a near-match id. Use integer "
            "ARGB colors like 4289721652 when possible. Every visible URL-bearing Canvas node "
            "must be unique: do not reuse a URL as two event urls, as two artifact urls, or as "
            "both an event url and an artifact url. If one source supports several subfindings, "
            "either summarize those subfindings inside one event or find distinct source URLs "
            "before emitting separate events. Do not emit "
            "one-artifact graphs for one-source events. Before you finish, audit the graph "
            "mutations you emitted: every new event should either be connected by at least one "
            "bridge to an existing Canvas event or connected to another new event in the same "
            "new cluster. Do not leave a semantically related event as a singleton; emit the "
            "missing bridge instead. Only emit an isolated singleton when the research result is "
            "truly unrelated to all existing Canvas content and you explicitly say so in a "
            "session.message.\n\n"
            f"{_research_tool_policy()}\n\n"
            f"Source policy:\n{SOURCE_POLICY_PROMPT}\n\n"
            f"Question: {prompt}"
        )


ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def _research_tool_policy() -> str:
    if os.getenv("AI_NEWS_REQUIRE_SEARCH_TRIAD", "1") != "1":
        return "Research tool policy: use the best available Hermes tools for the request."
    return (
        "Research tool policy: use SearXNG, Exa, and Browse.sh together when "
        "external research is needed. The project Hermes profile keeps "
        "Hermes web_search and web_extract backed by Exa, preserving Exa "
        "semantic discovery and richer retrieval. Also run SearXNG discovery "
        "passes through terminal/curl using AI_NEWS_SEARXNG_SEARCH_URL for broad "
        "meta-search URL and snippet diversity; the basic shape is "
        "`curl -sG \"$AI_NEWS_SEARXNG_SEARCH_URL\" --data-urlencode format=json "
        "--data-urlencode \"q=<query>\"`. Do not treat that as the only "
        "SearXNG mode: choose categories, engines, language, time_range, and "
        "pageno parameters when they fit the request. For AI/current-events "
        "research, consider parallel SearXNG passes over general, news, science, "
        "scientific publications, it, repos, and social media categories; use "
        "time_range=day/month/year for recency-sensitive questions, inspect "
        "unresponsive_engines, and adapt if an engine is rate-limited or blocked. "
        "If a SearXNG pass returns off-topic, generic, or empty results, retry "
        "with source-qualified queries such as site:official-domain/path, broader "
        "general+news categories, and a less restrictive time_range before using "
        "the pass as evidence. "
        "Then compare that frontier with Exa semantic results and promote "
        "primary sources. If an important candidate is dynamic, JavaScript-heavy, "
        "blocked to plain extraction, workflow-like, or only thinly captured by "
        "web_extract, use the "
        "Browse.sh CLI (`browse`) or Hermes browser tools to inspect it rather than "
        "discarding it. Browserbase cloud is optional; prefer local Browse.sh "
        "unless the cloud service is configured and the task justifies it."
    )


def _log_hermes_diagnostics(command: list[str], env: dict[str, str]) -> None:
    if os.getenv("AI_NEWS_HERMES_DIAGNOSTICS", "1") != "1":
        return
    logger.info(
        "Starting Hermes research: profile=%s home=%s provider=%s model=%s max_turns=%s",
        env.get("HERMES_PROFILE", ""),
        env.get("HERMES_HOME", ""),
        os.getenv("HERMES_PROVIDER", "opencode-go"),
        os.getenv("HERMES_MODEL", "mimo-v2.5-pro"),
        os.getenv("HERMES_MAX_TURNS", "12"),
    )
    logger.info(
        "Research tools configured: triad_required=%s searxng_url=%s exa_key=%s browse_cli=%s browserbase_key=%s",
        os.getenv("AI_NEWS_REQUIRE_SEARCH_TRIAD", "1"),
        env.get("AI_NEWS_SEARXNG_SEARCH_URL")
        or _searxng_search_url(env.get("AI_NEWS_SEARXNG_URL", "")),
        "set" if os.getenv("EXA_API_KEY") else "missing",
        shutil.which(os.getenv("BROWSE_CLI", "browse")) or "missing",
        "set" if os.getenv("BROWSERBASE_API_KEY") else "missing",
    )
    logger.info("Hermes command: %s", " ".join(_redact_command(command)))


def _log_hermes_line(line: str) -> None:
    if os.getenv("AI_NEWS_HERMES_LOG_LINES", "1") != "1":
        return
    max_chars = _env_int("AI_NEWS_HERMES_LOG_MAX_CHARS", 900)
    logger.info("Hermes stdout: %s", _truncate_for_log(_redact_text(line), max_chars))


def _searxng_search_url(url: str) -> str:
    root = (url or "http://127.0.0.1:8889").rstrip("/")
    if root.endswith("/search"):
        return root
    return f"{root}/search"


def _redact_command(command: list[str]) -> list[str]:
    redacted: list[str] = []
    skip_next = False
    for part in command:
        if skip_next:
            redacted.append("<redacted>")
            skip_next = False
            continue
        redacted.append(part)
        if part in {"--query", "-q", "--image"}:
            skip_next = True
    return redacted


SECRET_RE = re.compile(
    r"\b(?:sk|gsk|exa|hf|github_pat)[_-][A-Za-z0-9_\-]{12,}\b|"
    r"\b[A-Za-z0-9]{8}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{4}-[A-Za-z0-9]{12}\b"
)


def _redact_text(text: str) -> str:
    return SECRET_RE.sub("<redacted>", text)


def _truncate_for_log(text: str, max_chars: int) -> str:
    if max_chars <= 0 or len(text) <= max_chars:
        return text
    return f"{text[:max_chars]}... <truncated {len(text) - max_chars} chars>"


def _env_int(name: str, default: int) -> int:
    try:
        return int(os.getenv(name, str(default)))
    except ValueError:
        return default


def _clean_hermes_status_line(line: str) -> str:
    line = ANSI_ESCAPE_RE.sub("", line)
    line = line.replace("⏱", "").replace("—", "-")
    return " ".join(line.split())


def _harvest_state_graph_outputs(
    started_at: float, seen_outputs: set[str]
) -> list[AgentOutput]:
    if not HERMES_STATE_DB.exists():
        return []

    try:
        with sqlite3.connect(HERMES_STATE_DB) as connection:
            rows = connection.execute(
                """
                SELECT content
                FROM messages
                WHERE timestamp >= ?
                  AND content LIKE ?
                ORDER BY id
                """,
                (started_at - 2, f"%{EVENT_PREFIX}%"),
            ).fetchall()
    except sqlite3.Error:
        return []

    outputs: list[AgentOutput] = []
    for (content,) in rows:
        for text in _message_candidate_texts(content or ""):
            for line in _event_lines(text):
                try:
                    output = normalize_agent_output(parse_agent_output(line))
                except (KeyError, ValueError, TypeError, json.JSONDecodeError):
                    continue
                if _mark_seen_output(seen_outputs, output):
                    outputs.append(output)
    return outputs


def _message_candidate_texts(content: str) -> list[str]:
    texts = [content]
    try:
        decoded = json.loads(content)
    except json.JSONDecodeError:
        return texts
    if isinstance(decoded, dict):
        output = decoded.get("output")
        if isinstance(output, str):
            texts.append(output)
    return texts


def _event_lines(text: str) -> list[str]:
    lines = []
    for raw_line in text.splitlines():
        index = raw_line.find(EVENT_PREFIX)
        if index == -1:
            continue
        lines.append(raw_line[index:].strip())
    return lines


def _mark_seen_output(seen_outputs: set[str], output: AgentOutput) -> bool:
    key = f"{output.type}:{json.dumps(output.data, sort_keys=True)}"
    if key in seen_outputs:
        return False
    seen_outputs.add(key)
    return True


def _should_schedule_voice_note(
    output: AgentOutput,
    *,
    last_spoken_at: float,
    spoken_count: int,
    now: float | None = None,
) -> bool:
    if output.type != "voice.note" or not _voice_note_text(output):
        return False
    if spoken_count >= _voice_note_limit():
        return False
    now = time.time() if now is None else now
    return now - last_spoken_at >= _voice_note_interval()


def _voice_note_text(output: AgentOutput) -> str:
    if isinstance(output.data, dict):
        text = str(output.data.get("message") or "").strip()
    else:
        text = str(output.data or "").strip()
    return _truncate_voice_note(text)


def _truncate_voice_note(text: str) -> str:
    words = text.split()
    limit = _voice_note_max_chars()
    if len(text) <= limit:
        return text
    result = ""
    for word in words:
        candidate = f"{result} {word}".strip()
        if len(candidate) > limit:
            break
        result = candidate
    return result.rstrip(" ,;:") + "."


def _voice_note_interval() -> float:
    try:
        return max(0.0, float(os.getenv("AI_NEWS_VOICE_NOTE_INTERVAL", "35")))
    except ValueError:
        return 35.0


def _voice_note_limit() -> int:
    try:
        return max(0, int(os.getenv("AI_NEWS_VOICE_NOTE_LIMIT", "2")))
    except ValueError:
        return 2


def _voice_note_max_age_seconds() -> float:
    try:
        return max(0.0, float(os.getenv("AI_NEWS_VOICE_NOTE_MAX_AGE", "12")))
    except ValueError:
        return 12.0


def _voice_note_max_chars() -> int:
    try:
        return max(40, int(os.getenv("AI_NEWS_VOICE_NOTE_MAX_CHARS", "110")))
    except ValueError:
        return 110


def _schedule_voice(coro) -> None:
    task = asyncio.create_task(coro)

    def _log_voice_result(done: asyncio.Task) -> None:
        try:
            message = done.result()
        except Exception as error:
            logger.warning("Hermes voice notifier crashed: %s", error, exc_info=True)
            return
        if message:
            logger.warning(message)

    task.add_done_callback(_log_voice_result)
