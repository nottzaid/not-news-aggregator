from __future__ import annotations

import asyncio
import logging
import os
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import AsyncIterator

from fastapi import FastAPI, File, Query, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse, StreamingResponse

from .config import load_private_env
from .fixtures import FIXTURE_BRIDGES, FIXTURE_EVENTS
from .graph_store import GraphStore
from .hermes_runner import HermesRunner
from .sse import encode_sse


load_private_env()
logger = logging.getLogger(__name__)

app = FastAPI(title="AI News Canvas Backend")
graph_store = GraphStore()
app.add_middleware(
    CORSMiddleware,
    allow_origins=os.getenv("CORS_ALLOW_ORIGINS", "*").split(","),
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/health")
async def health() -> dict[str, str]:
    return {"status": "ok"}


@app.get("/graph/stream")
async def graph_stream() -> StreamingResponse:
    return StreamingResponse(
        _stored_graph_stream(), media_type="text/event-stream"
    )


@app.delete("/graph")
async def clear_graph() -> dict[str, str]:
    graph_store.clear()
    return {"status": "cleared"}


@app.get("/research/stream")
async def research_stream(
    prompt: str = Query("What is there to know about the Anthropic-SpaceX deal?"),
) -> StreamingResponse:
    return StreamingResponse(_research_graph_stream(prompt), media_type="text/event-stream")


@app.post("/audio/transcribe")
async def transcribe_audio(audio: UploadFile = File(...)) -> JSONResponse:
    api_key = os.getenv("GROQ_API_KEY")
    if not api_key:
        return JSONResponse({"error": "GROQ_API_KEY is not configured."}, status_code=503)

    suffix = os.path.splitext(audio.filename or "recording.webm")[1] or ".webm"
    try:
        with tempfile.NamedTemporaryFile(suffix=suffix) as handle:
            handle.write(await audio.read())
            handle.flush()
            transcript = _transcribe_with_groq(
                handle.name, audio.filename or "recording.webm", api_key
            )
    except Exception as error:
        logger.exception("Audio transcription failed")
        return _transcription_error_response(error)
    return JSONResponse({"text": transcript})


async def _fixture_graph_stream() -> AsyncIterator[str]:
    yield encode_sse("session.message", {"message": "Loading the local fixture graph stream."})
    for event in FIXTURE_EVENTS:
        yield encode_sse("event.upsert", event.model_dump(by_alias=True))
        await asyncio.sleep(0.08)
    for bridge in FIXTURE_BRIDGES:
        yield encode_sse("bridge.upsert", bridge.model_dump(by_alias=True))
        await asyncio.sleep(0.05)
    yield encode_sse("session.done", {"message": "Fixture graph stream complete."})


async def _stored_graph_stream() -> AsyncIterator[str]:
    if not graph_store.has_data():
        yield encode_sse("session.message", {"message": "Canvas is empty."})
        yield encode_sse("session.done", {"message": "Empty canvas loaded."})
        return

    yield encode_sse("session.message", {"message": "Loading the saved graph."})
    async for item in _stored_graph_mutation_stream():
        yield item
    yield encode_sse("session.done", {"message": "Saved graph loaded."})


async def _stored_graph_mutation_stream() -> AsyncIterator[str]:
    events = graph_store.list_events()
    bridges = graph_store.list_bridges()
    for event in events:
        yield encode_sse("event.upsert", event)
        await asyncio.sleep(0)
    for bridge in bridges:
        yield encode_sse("bridge.upsert", bridge)
        await asyncio.sleep(0)


async def _research_graph_stream(prompt: str) -> AsyncIterator[str]:
    yield encode_sse("session.started", {"message": "Research session started."})
    if graph_store.has_data():
        yield encode_sse(
            "session.message",
            {"message": "Restoring the saved canvas before research."},
        )
        async for item in _stored_graph_mutation_stream():
            yield item

    saw_mutation = False
    async for output in HermesRunner().stream_research_updates(prompt):
        if output.type == "event.upsert" and isinstance(output.data, dict):
            output = output.__class__(
                type=output.type, data=graph_store.upsert_event(output.data)
            )
            saw_mutation = True
        elif output.type == "bridge.upsert" and isinstance(output.data, dict):
            bridge = graph_store.upsert_bridge(output.data)
            if bridge is None:
                continue
            output = output.__class__(type=output.type, data=bridge)
            saw_mutation = True
        yield encode_sse(output.type, output.data)
        await asyncio.sleep(0)
    yield encode_sse("session.done", {"message": "Research session complete."})


def _transcribe_with_groq(path: str, filename: str, api_key: str) -> str:
    request = _build_groq_transcription_request(path, filename, api_key)
    try:
        with urllib.request.urlopen(request, timeout=45) as response:
            body = response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        detail = error.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"Groq transcription failed: {detail}") from error
    import json

    decoded = json.loads(body)
    return decoded.get("text", "")


def _build_groq_transcription_request(
    path: str, filename: str, api_key: str
) -> urllib.request.Request:
    model = os.getenv("STT_GROQ_MODEL") or os.getenv("GROQ_WHISPER_MODEL", "whisper-large-v3-turbo")
    boundary = "ai-news-canvas-boundary"
    with open(path, "rb") as file:
        audio_bytes = file.read()
    fields = [
        _multipart_field(boundary, "model", model),
        _multipart_file(boundary, "file", filename, audio_bytes),
        f"--{boundary}--\r\n".encode("utf-8"),
    ]
    return urllib.request.Request(
        "https://api.groq.com/openai/v1/audio/transcriptions",
        data=b"".join(fields),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Accept": "application/json",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "User-Agent": "AI-News-Canvas/0.1 (+https://localhost)",
        },
        method="POST",
    )


def _transcription_error_response(error: Exception) -> JSONResponse:
    return JSONResponse({"error": str(error)}, status_code=502)


def _multipart_field(boundary: str, name: str, value: str) -> bytes:
    return (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="{name}"\r\n\r\n'
        f"{value}\r\n"
    ).encode("utf-8")


def _multipart_file(boundary: str, name: str, filename: str, content: bytes) -> bytes:
    safe_filename = urllib.parse.quote(filename)
    return (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="{name}"; filename="{safe_filename}"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode("utf-8") + content + b"\r\n"
