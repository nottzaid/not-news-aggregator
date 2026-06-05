from __future__ import annotations

import json
import os
import urllib.error
import urllib.request


class KokoroStatusModel:
    def __init__(self) -> None:
        self.base_url = os.getenv("KOKORO_BASE_URL", "").rstrip("/")
        self.model = os.getenv("KOKORO_MODEL", "kokoro")
        self.api_key = os.getenv("KOKORO_API_KEY", "")

    def summarize(self, raw_update: str) -> str:
        if not self.base_url:
            return raw_update

        return self.complete(
            [
                {
                    "role": "system",
                    "content": (
                        "Summarize this backend research-agent update for a live Canvas UI. "
                        "Use one concise sentence, no markdown."
                    ),
                },
                {"role": "user", "content": raw_update},
            ],
            fallback=raw_update,
            temperature=0.2,
            max_tokens=80,
        )

    def complete(
        self,
        messages: list[dict[str, str]],
        *,
        fallback: str,
        temperature: float = 0.45,
        max_tokens: int = 120,
    ) -> str:
        if not self.base_url:
            return fallback

        payload = {
            "model": self.model,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
        }
        request = urllib.request.Request(
            f"{self.base_url}/chat/completions",
            data=json.dumps(payload).encode("utf-8"),
            headers={
                "Content-Type": "application/json",
                **({"Authorization": f"Bearer {self.api_key}"} if self.api_key else {}),
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=8) as response:
                body = json.loads(response.read().decode("utf-8"))
            content = body["choices"][0]["message"]["content"].strip()
            return content or fallback
        except (KeyError, OSError, TimeoutError, urllib.error.URLError, json.JSONDecodeError):
            return fallback
