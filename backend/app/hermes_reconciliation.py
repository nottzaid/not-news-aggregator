from __future__ import annotations

import asyncio
import json
import os
from typing import Any

from .config import PROJECT_ROOT
from .hermes_runner import HERMES_PROFILE, HermesRunner


class HermesReconciliationRunner:
    async def reconcile(self, context: dict[str, Any]) -> list[dict[str, Any]]:
        result = await self._invoke(self._prompt(context))
        actions = result.get("actions")
        if not isinstance(actions, list):
            raise ValueError("Hermes reconciliation returned no actions.")
        return [dict(action) for action in actions if isinstance(action, dict)]

    async def review_destination(self, context: dict[str, Any]) -> dict[str, Any]:
        return await self._invoke(
            "Review a researcher-authored relationship created by dragging an "
            "event on a research Canvas. The relationship is authoritative: "
            "you may advise but not mutate it. Return JSON only as "
            '{"verdict":"supported|no_concerns|concern","summary":"brief '
            'evidence-based explanation","proposedLabel":"optional"}. Context:\n'
            + json.dumps(context, ensure_ascii=False, separators=(",", ":"))
        )

    async def _invoke(self, prompt: str) -> dict[str, Any]:
        if os.getenv("AI_NEWS_ENABLE_HERMES", "0") != "1":
            raise RuntimeError("Hermes reconciliation is disabled.")

        runner = HermesRunner()
        command = [
            "hermes",
            "--profile",
            HERMES_PROFILE,
            "chat",
            "--query",
            prompt,
            "--provider",
            os.getenv(
                "HERMES_RECONCILIATION_PROVIDER",
                os.getenv("HERMES_PROVIDER", "opencode-go"),
            ),
            "--model",
            os.getenv(
                "HERMES_RECONCILIATION_MODEL",
                os.getenv("HERMES_MODEL", "mimo-v2.5-pro"),
            ),
            "--quiet",
            "--yolo",
            "--source",
            "ai-news-canvas-reconciliation",
            "--max-turns",
            os.getenv("HERMES_RECONCILIATION_MAX_TURNS", "4"),
        ]
        process = await asyncio.create_subprocess_exec(
            *command,
            cwd=PROJECT_ROOT,
            env=runner._hermes_env(),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
        )
        stdout, _ = await asyncio.wait_for(
            process.communicate(),
            timeout=float(os.getenv("HERMES_RECONCILIATION_TIMEOUT", "45")),
        )
        if process.returncode != 0:
            detail = stdout.decode("utf-8", errors="replace").strip()
            raise RuntimeError(
                f"Hermes reconciliation exited with status {process.returncode}: "
                f"{detail[-800:]}"
            )
        return _extract_json(stdout.decode("utf-8", errors="replace"))

    def _prompt(self, context: dict[str, Any]) -> str:
        return (
            "You are reconciling the OLD relationships left behind after a "
            "researcher dragged an event elsewhere on a research Canvas. The "
            "new destination is authoritative and outside your authority. "
            "Decide every supplied old relationship as keep, remove, or amend. "
            "Keep a relationship only when its semantic claim remains valid "
            "despite the researcher's move. Remove grouping-like or misleading "
            "relationships. Amend only when a clearer label preserves a valid "
            "claim. Return JSON only, exactly "
            '{"actions":[{"bridgeId":"...","action":"keep|remove|amend",'
            '"reason":"...","label":"required only for amend"}]}. '
            "Use each bridgeId exactly once and invent no IDs. Do not use web "
            "research or mutate files. Context:\n"
            + json.dumps(context, ensure_ascii=False, separators=(",", ":"))
        )


def _extract_json(output: str) -> dict[str, Any]:
    text = output.strip()
    decoder = json.JSONDecoder()
    for index, character in enumerate(text):
        if character != "{":
            continue
        try:
            value, _ = decoder.raw_decode(text[index:])
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and (
            "actions" in value or "verdict" in value
        ):
            return value
    raise ValueError("Hermes reconciliation did not return valid JSON.")
