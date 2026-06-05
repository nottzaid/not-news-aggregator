from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

from .schemas import EventBridgeDto, ResearchEventDto


EVENT_PREFIX = "AI_NEWS_EVENT:"


@dataclass(frozen=True)
class AgentOutput:
    type: str
    data: Any


def parse_agent_output(line: str) -> AgentOutput:
    if not line.startswith(EVENT_PREFIX):
        return AgentOutput(type="session.message", data={"message": line})

    raw = line.removeprefix(EVENT_PREFIX).strip()
    payload = json.loads(raw)
    event_type = payload["type"]
    data = payload.get("data", {})
    if event_type not in {
        "event.upsert",
        "bridge.upsert",
        "session.message",
        "session.error",
        "session.done",
        "voice.note",
    }:
        raise ValueError(f"Unsupported agent event type: {event_type}")
    return AgentOutput(type=event_type, data=data)


def normalize_agent_output(output: AgentOutput) -> AgentOutput:
    if output.type == "event.upsert" and isinstance(output.data, dict):
        return AgentOutput(type=output.type, data=_normalize_event(output.data))
    if output.type == "bridge.upsert" and isinstance(output.data, dict):
        return AgentOutput(
            type=output.type,
            data=EventBridgeDto.model_validate(output.data).model_dump(by_alias=True),
        )
    return output


def _normalize_event(payload: dict[str, Any]) -> dict[str, Any]:
    next_payload = dict(payload)
    color = next_payload.get("color")
    if isinstance(color, str):
        normalized = color.removeprefix("#")
        if len(normalized) == 6:
            normalized = "ff" + normalized
        next_payload["color"] = int(normalized, 16)

    source_label = str(next_payload.get("sourceLabel") or "source")
    artifacts = []
    for artifact in next_payload.get("artifacts") or []:
        if isinstance(artifact, str):
            artifacts.append(
                {
                    "text": source_label,
                    "source": source_label,
                    "url": artifact,
                }
            )
            continue
        if not isinstance(artifact, dict):
            continue
        text = artifact.get("text") or artifact.get("label") or artifact.get("title") or "Source"
        source = artifact.get("source") or source_label
        artifacts.append(
            {
                "text": str(text),
                "source": str(source),
                "url": str(artifact["url"]),
            }
        )
    next_payload["artifacts"] = artifacts
    return ResearchEventDto.model_validate(next_payload).model_dump(by_alias=True)
