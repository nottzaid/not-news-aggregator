import asyncio
import json
import sqlite3
import tempfile
import time
from pathlib import Path

from app.agent_events import normalize_agent_output, parse_agent_output
from app.fixtures import FIXTURE_BRIDGES, FIXTURE_EVENTS
from app.graph_store import GraphStore
from app.hermes_runner import (
    HermesRunner,
    _clean_hermes_status_line,
    _harvest_state_graph_outputs,
    _redact_command,
    _redact_text,
    _research_tool_policy,
    _should_schedule_voice_note,
    _truncate_for_log,
    _voice_note_text,
)
from app.hermes_voice import (
    HermesVoiceNotifier,
    _RECENT_UTTERANCES,
    _briefing_context,
    _clean_spoken_text,
    _remember_spoken,
    _source_root_from_executable,
    _was_recently_spoken,
)
from app import hermes_runner as hermes_runner_module
from app import main as main_module
from app.main import (
    _build_groq_transcription_request,
    _fixture_graph_stream,
    _research_graph_stream,
    _stored_graph_stream,
    _transcription_error_response,
    clear_graph,
)
from app.sse import encode_sse


def test_sse_event_encoding():
    frame = encode_sse("event.upsert", {"id": "spacex"})

    assert frame.startswith("event: event.upsert\n")
    assert 'data: {"id":"spacex"}' in frame
    assert frame.endswith("\n\n")


def test_fixture_graph_stream_emits_required_mutations():
    async def collect():
        return [frame async for frame in _fixture_graph_stream()]

    frames = asyncio.run(collect())
    events = [_event_type(frame) for frame in frames]

    assert "event.upsert" in events
    assert "bridge.upsert" in events
    assert events[-1] == "session.done"
    assert events.count("event.upsert") == len(FIXTURE_EVENTS)
    assert events.count("bridge.upsert") == len(FIXTURE_BRIDGES)


def test_fixture_event_shape_uses_canvas_model_aliases():
    payload = FIXTURE_EVENTS[0].model_dump(by_alias=True)

    assert "sourceLabel" in payload
    assert "source_label" not in payload
    assert payload["artifacts"][0]["url"].startswith("https://")


def test_agent_event_parser_accepts_graph_mutation_lines():
    output = parse_agent_output(
        'AI_NEWS_EVENT: {"type":"bridge.upsert","data":{"from":"a","to":"b","label":"related"}}'
    )

    assert output.type == "bridge.upsert"
    assert output.data["from"] == "a"


def test_agent_event_parser_accepts_autonomous_voice_notes():
    output = parse_agent_output(
        'AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"I found a useful source and I am checking it now."}}'
    )

    assert output.type == "voice.note"
    assert output.data["message"].startswith("I found")


def test_agent_event_normalizer_accepts_label_artifacts_and_hex_color():
    output = normalize_agent_output(
        parse_agent_output(
            'AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"a","title":"A","date":"Jun 4, 2026","color":"#76B900","summary":"S","sourceLabel":"Nvidia","artifacts":[{"label":"Press Release","url":"https://example.com"}]}}'
        )
    )

    assert output.data["color"] == 0xFF76B900
    assert output.data["artifacts"][0] == {
        "text": "Press Release",
        "source": "Nvidia",
        "url": "https://example.com",
    }


def test_agent_event_normalizer_accepts_string_artifact_urls():
    output = normalize_agent_output(
        parse_agent_output(
            'AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"a","title":"A","date":"Jun 4, 2026","color":"#76B900","summary":"S","sourceLabel":"Nvidia","artifacts":["https://example.com/source"],"url":"https://example.com/source"}}'
        )
    )

    assert output.data["artifacts"][0] == {
        "text": "Nvidia",
        "source": "Nvidia",
        "url": "https://example.com/source",
    }


def test_graph_store_persists_mutations(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    event = _stored_event("event-a")
    bridge = {"from": "event-a", "to": "event-b", "label": "related"}
    store.upsert_event(event)
    store.upsert_event(_stored_event("event-b"))
    store.upsert_bridge(bridge)

    assert (tmp_path / "graph.sqlite").exists()
    assert store.list_events() == [event, _stored_event("event-b")]
    assert store.list_bridges() == [bridge]
    assert store.has_data()


def test_graph_store_hides_bridges_with_missing_endpoints(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(_stored_event("event-a"))
    store.upsert_bridge({"from": "event-a", "to": "missing", "label": "related"})

    assert store.list_bridges() == []


def test_graph_store_dedupes_events_by_date_and_source_url(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    canonical = _stored_event(
        "deepswe-benchmark-launch-2026-05-26",
        title="Datacurve releases DeepSWE benchmark",
        url="https://deepswe.datacurve.ai/",
    )
    duplicate = _stored_event(
        "deepswe-benchmark-launch-20260526",
        title="DeepSWE benchmark launches",
        url="https://deepswe.datacurve.ai",
    )
    store.upsert_event(canonical)

    saved = store.upsert_event(duplicate)

    assert saved == canonical
    assert [event["id"] for event in store.list_events()] == [
        "deepswe-benchmark-launch-2026-05-26"
    ]


def test_graph_store_rewrites_bridges_through_event_aliases(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(
        _stored_event(
            "canonical-event",
            title="Canonical",
            url="https://example.com/canonical",
        )
    )
    store.upsert_event(
        _stored_event(
            "duplicate-event",
            title="Duplicate",
            url="https://example.com/canonical/",
        )
    )
    store.upsert_event(_stored_event("target-event"))

    saved = store.upsert_bridge(
        {"from": "duplicate-event", "to": "target-event", "label": "related — strongly"}
    )

    assert saved == {
        "from": "canonical-event",
        "to": "target-event",
        "label": "related - strongly",
    }
    assert store.list_bridges() == [saved]


def test_graph_store_rejects_self_loop_bridges(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(_stored_event("event-a"))

    saved = store.upsert_bridge(
        {"from": "event-a", "to": "event-a", "label": "same event"}
    )

    assert saved is None
    assert store.list_bridges() == []


def test_graph_store_deletes_event_and_connected_bridges(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(_stored_event("event-a"))
    store.upsert_event(_stored_event("event-b"))
    store.upsert_bridge({"from": "event-a", "to": "event-b", "label": "related"})

    store.delete_event("event-a")

    assert [event["id"] for event in store.list_events()] == ["event-b"]
    assert store.list_bridges() == []


def test_graph_store_clear_removes_events_bridges_and_aliases(tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(
        _stored_event("canonical-event", url="https://example.com/canonical")
    )
    store.upsert_event(
        _stored_event(
            "duplicate-event",
            url="https://example.com/canonical/",
        )
    )
    store.upsert_event(_stored_event("target-event"))
    store.upsert_bridge(
        {"from": "duplicate-event", "to": "target-event", "label": "related"}
    )

    store.clear()

    assert store.list_events() == []
    assert store.list_bridges() == []
    assert store.upsert_bridge(
        {"from": "duplicate-event", "to": "target-event", "label": "related"}
    ) is None


def test_clear_graph_endpoint_clears_store(monkeypatch, tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(_stored_event("saved-event"))
    monkeypatch.setattr(main_module, "graph_store", store)

    response = asyncio.run(clear_graph())

    assert response == {"status": "cleared"}
    assert store.has_data() is False


def test_graph_stream_emits_empty_canvas_when_no_saved_data(monkeypatch, tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    monkeypatch.setattr(main_module, "graph_store", store)

    async def collect():
        return [frame async for frame in _stored_graph_stream()]

    frames = asyncio.run(collect())

    assert [_event_type(frame) for frame in frames] == [
        "session.message",
        "session.done",
    ]
    assert not any("event.upsert" in frame for frame in frames)
    assert not any('"id":"spacex"' in frame for frame in frames)


def test_graph_stream_replays_saved_graph(monkeypatch, tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(_stored_event("saved-event"))
    monkeypatch.setattr(main_module, "graph_store", store)

    async def collect():
        return [frame async for frame in _stored_graph_stream()]

    frames = asyncio.run(collect())

    assert _event_type(frames[0]) == "session.message"
    assert _event_type(frames[1]) == "event.upsert"
    assert '"id":"saved-event"' in frames[1]
    assert not any('"id":"spacex"' in frame for frame in frames)


def test_research_stream_replays_saved_graph_before_agent(monkeypatch, tmp_path: Path):
    store = GraphStore(tmp_path / "graph.sqlite")
    store.upsert_event(_stored_event("saved-event"))
    monkeypatch.setattr(main_module, "graph_store", store)

    class EmptyRunner:
        async def stream_research_updates(self, prompt: str):
            if False:
                yield None

    monkeypatch.setattr(main_module, "HermesRunner", EmptyRunner)

    async def collect():
        return [frame async for frame in _research_graph_stream("prompt")]

    frames = asyncio.run(collect())

    assert _event_type(frames[0]) == "session.started"
    assert _event_type(frames[1]) == "session.message"
    assert _event_type(frames[2]) == "event.upsert"
    assert '"id":"saved-event"' in frames[2]
    assert not any('"id":"spacex"' in frame for frame in frames)


def test_audio_transcription_errors_return_json():
    response = _transcription_error_response(RuntimeError("provider rejected audio"))

    assert response.status_code == 502
    assert json.loads(response.body) == {"error": "provider rejected audio"}


def test_groq_transcription_request_uses_api_friendly_headers():
    with tempfile.NamedTemporaryFile(suffix=".wav") as audio:
        audio.write(b"RIFF....WAVE")
        audio.flush()

        request = _build_groq_transcription_request(
            audio.name, "recording.wav", "test-key"
        )

    assert request.get_method() == "POST"
    assert request.headers["Authorization"] == "Bearer test-key"
    assert request.headers["Accept"] == "application/json"
    assert request.headers["User-agent"].startswith("AI-News-Canvas/")
    assert "multipart/form-data" in request.headers["Content-type"]
    assert b'name="model"' in request.data
    assert b'name="file"; filename="recording.wav"' in request.data


def test_hermes_status_line_cleanup_removes_terminal_formatting():
    assert (
        _clean_hermes_status_line("\x1b[2m  ⏱ Timeout — denying command\x1b[0m")
        == "Timeout - denying command"
    )


def test_harvests_graph_events_from_hermes_tool_output(monkeypatch, tmp_path: Path):
    state_db = tmp_path / "state.db"
    started_at = time.time()
    with sqlite3.connect(state_db) as connection:
        connection.execute(
            """
            CREATE TABLE messages (
                id INTEGER PRIMARY KEY,
                content TEXT,
                timestamp REAL
            )
            """
        )
        connection.execute(
            "INSERT INTO messages (content, timestamp) VALUES (?, ?)",
            (
                json.dumps(
                    {
                        "output": (
                            'AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"kimi-k26","title":"Kimi K2.6","date":"2026-04-20","color":4283215696,"summary":"S","sourceLabel":"Moonshot","artifacts":["https://example.com/kimi"],"url":"https://example.com/kimi"}}\n'
                            'AI_NEWS_EVENT: {"type":"bridge.upsert","data":{"from":"kimi-k25","to":"kimi-k26","label":"successor"}}\n'
                        )
                    }
                ),
                started_at + 0.1,
            ),
        )
    monkeypatch.setattr(hermes_runner_module, "HERMES_STATE_DB", state_db)

    outputs = _harvest_state_graph_outputs(started_at, set())

    assert [output.type for output in outputs] == ["event.upsert", "bridge.upsert"]
    assert outputs[0].data["id"] == "kimi-k26"
    assert outputs[0].data["artifacts"][0]["url"] == "https://example.com/kimi"


def test_hermes_command_uses_noninteractive_approval_mode():
    command = HermesRunner()._hermes_command("research")

    assert "--yolo" in command
    assert "--quiet" not in command
    assert "--ignore-rules" not in command
    assert "--ignore-user-config" not in command


def test_hermes_env_uses_project_profile_home():
    env = HermesRunner()._hermes_env()

    assert env["HERMES_HOME"].endswith("/.hermes/profiles/ainews")
    assert env["HERMES_PROFILE"] == "ainews"


def test_hermes_diagnostics_redact_prompt_and_secrets(monkeypatch, caplog):
    caplog.set_level("INFO")
    assert _redact_command(["hermes", "chat", "--query", "secret prompt"]) == [
        "hermes",
        "chat",
        "--query",
        "<redacted>",
    ]
    assert _redact_text("key sk-testsecretvalue1234567890") == "key <redacted>"
    assert _redact_text("exa f134b7de-3bb8-400a-a054-f2f186ef77c5") == "exa <redacted>"
    assert _truncate_for_log("abcdef", 4) == "abcd... <truncated 2 chars>"
    monkeypatch.setenv("AI_NEWS_HERMES_LOG_MAX_CHARS", "24")

    hermes_runner_module._log_hermes_line(
        "token sk-testsecretvalue1234567890 and extra text"
    )

    assert "<redacted>" in caplog.text


def test_research_prompt_requires_search_triad_by_default(monkeypatch):
    monkeypatch.delenv("AI_NEWS_REQUIRE_SEARCH_TRIAD", raising=False)

    prompt = HermesRunner()._research_prompt("latest model news")

    assert "SearXNG" in prompt
    assert "Exa" in prompt
    assert "Browse.sh" in prompt
    assert "AI_NEWS_SEARXNG_SEARCH_URL" in prompt
    assert "Exa semantic discovery" in prompt
    assert "source-qualified queries" in prompt


def test_hermes_env_exposes_searxng_search_endpoint(monkeypatch):
    monkeypatch.delenv("AI_NEWS_SEARXNG_SEARCH_URL", raising=False)
    monkeypatch.setenv("AI_NEWS_SEARXNG_URL", "http://127.0.0.1:8889")

    env = HermesRunner()._hermes_env()

    assert env["AI_NEWS_SEARXNG_SEARCH_URL"] == "http://127.0.0.1:8889/search"


def test_research_tool_policy_can_be_relaxed(monkeypatch):
    monkeypatch.setenv("AI_NEWS_REQUIRE_SEARCH_TRIAD", "0")

    assert "best available Hermes tools" in _research_tool_policy()


def test_voice_note_policy_is_agent_chosen_and_throttled(monkeypatch):
    monkeypatch.setenv("AI_NEWS_VOICE_NOTE_INTERVAL", "10")
    monkeypatch.setenv("AI_NEWS_VOICE_NOTE_LIMIT", "1")
    output = parse_agent_output(
        'AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"This is worth saying aloud."}}'
    )

    assert _should_schedule_voice_note(
        output, last_spoken_at=100, spoken_count=0, now=111
    )
    assert not _should_schedule_voice_note(
        output, last_spoken_at=105, spoken_count=0, now=111
    )
    assert not _should_schedule_voice_note(
        output, last_spoken_at=100, spoken_count=1, now=111
    )


def test_voice_notes_are_shortened_for_live_speech(monkeypatch):
    monkeypatch.setenv("AI_NEWS_VOICE_NOTE_MAX_CHARS", "64")
    output = parse_agent_output(
        'AI_NEWS_EVENT: {"type":"voice.note","data":{"message":"I found a useful source about the release and I am checking whether it changes the canvas relationships."}}'
    )

    text = _voice_note_text(output)

    assert len(text) <= 65
    assert text.endswith(".")


def test_voice_briefing_context_uses_streamed_graph_outputs():
    outputs = [
        parse_agent_output(
            'AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"a","title":"Alpha","date":"Jun 4, 2026","color":4280000000,"summary":"S","sourceLabel":"Test","artifacts":[]}}'
        ),
        parse_agent_output(
            'AI_NEWS_EVENT: {"type":"bridge.upsert","data":{"from":"a","to":"b","label":"related"}}'
        ),
    ]

    context = _briefing_context("research alpha", outputs, saw_mutation=True)

    assert "research alpha" in context
    assert "Alpha" in context
    assert "Relationships added or updated: 1" in context


def test_voice_text_cleanup_removes_markdown_and_urls():
    cleaned = _clean_spoken_text("**Done**: see https://example.com and `code`")

    assert cleaned == "Done : see and code"


def test_voice_notifier_respects_disable_switch(monkeypatch):
    monkeypatch.setenv("AI_NEWS_ENABLE_VOICE", "0")

    async def run():
        return await HermesVoiceNotifier().announce_start("prompt")

    assert asyncio.run(run()) is None


def test_voice_duplicate_suppression_uses_cleaned_text(monkeypatch):
    monkeypatch.setenv("AI_NEWS_VOICE_DEDUPE_SECONDS", "20")
    _RECENT_UTTERANCES.clear()

    _remember_spoken("Research complete: I updated the canvas.", now=100)

    assert _was_recently_spoken("research complete i updated the canvas", now=110)
    assert not _was_recently_spoken("research complete i updated the canvas", now=130)


def test_hermes_source_root_can_be_derived_from_wrapper(tmp_path: Path):
    root = tmp_path / "hermes-agent"
    (root / "tools").mkdir(parents=True)
    (root / "tools" / "tts_tool.py").write_text("", encoding="utf-8")
    (root / "tools" / "voice_mode.py").write_text("", encoding="utf-8")
    executable = tmp_path / "hermes"
    executable.write_text(
        f'#!/usr/bin/env bash\nexec "{root}/venv/bin/hermes" "$@"\n',
        encoding="utf-8",
    )

    assert _source_root_from_executable(executable) == root


def _event_type(frame: str) -> str:
    first_line = frame.splitlines()[0]
    return first_line.removeprefix("event: ")


def _stored_event(
    event_id: str,
    *,
    title: str = "Event A",
    url: str | None = None,
) -> dict[str, object]:
    return {
        "id": event_id,
        "title": title,
        "date": "Jun 3, 2026",
        "color": 4280000000,
        "summary": "Stored event.",
        "sourceLabel": "Test",
        "artifacts": [],
        "url": url or f"https://example.com/{event_id}",
    }
