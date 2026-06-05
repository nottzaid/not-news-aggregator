from __future__ import annotations

import json
from collections.abc import AsyncIterator
from typing import Any


def encode_sse(event: str, data: Any) -> str:
    if not isinstance(data, str):
        data = json.dumps(data, separators=(",", ":"))
    lines = [f"event: {event}"]
    lines.extend(f"data: {line}" for line in data.splitlines() or [""])
    return "\n".join(lines) + "\n\n"


async def with_heartbeat(events: AsyncIterator[str]) -> AsyncIterator[str]:
    async for event in events:
        yield event
