from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any

from .config import PROJECT_ROOT


class GraphStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or PROJECT_ROOT / "backend" / "data" / "graph.sqlite"
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._init()

    def upsert_event(self, payload: dict[str, Any]) -> dict[str, Any]:
        with self._connect() as connection:
            canonical_id = self._canonical_event_id(connection, payload)
            if canonical_id != payload["id"]:
                connection.execute(
                    """
                    INSERT INTO event_aliases (alias, canonical_id)
                    VALUES (?, ?)
                    ON CONFLICT(alias) DO UPDATE SET canonical_id = excluded.canonical_id
                    """,
                    (payload["id"], canonical_id),
                )
                row = connection.execute(
                    "SELECT payload FROM events WHERE id = ?", (canonical_id,)
                ).fetchone()
                if row is not None:
                    return json.loads(row[0])
                payload = {**payload, "id": canonical_id}
            payload = _dedupe_artifacts_for_event(connection, payload)
            connection.execute(
                """
                INSERT INTO events (id, payload)
                VALUES (?, ?)
                ON CONFLICT(id) DO UPDATE SET payload = excluded.payload
                """,
                (payload["id"], json.dumps(payload, separators=(",", ":"))),
            )
        return payload

    def upsert_bridge(self, payload: dict[str, Any]) -> dict[str, Any] | None:
        with self._connect() as connection:
            from_id = self._resolve_event_id(connection, payload["from"])
            to_id = self._resolve_event_id(connection, payload["to"])
            if from_id is None or to_id is None:
                return None
            if from_id == to_id:
                return None
            label = _normalize_bridge_label(str(payload["label"]))
            payload = {**payload, "from": from_id, "to": to_id, "label": label}
            key = f"{from_id}::{to_id}::{_bridge_key_label(label)}"
            connection.execute(
                """
                INSERT INTO bridges (id, payload)
                VALUES (?, ?)
                ON CONFLICT(id) DO UPDATE SET payload = excluded.payload
                """,
                (key, json.dumps(payload, separators=(",", ":"))),
            )
        return payload

    def list_events(self) -> list[dict[str, Any]]:
        with self._connect() as connection:
            return self._list_events(connection)

    def list_bridges(self) -> list[dict[str, Any]]:
        with self._connect() as connection:
            event_ids = {
                row[0] for row in connection.execute("SELECT id FROM events").fetchall()
            }
            rows = connection.execute(
                "SELECT payload FROM bridges ORDER BY rowid"
            ).fetchall()
        bridges = [json.loads(row[0]) for row in rows]
        return [
            bridge
            for bridge in bridges
            if bridge.get("from") in event_ids and bridge.get("to") in event_ids
        ]

    def has_data(self) -> bool:
        with self._connect() as connection:
            event_count = connection.execute(
                "SELECT COUNT(*) FROM events"
            ).fetchone()[0]
            bridge_count = connection.execute(
                "SELECT COUNT(*) FROM bridges"
            ).fetchone()[0]
        return bool(event_count or bridge_count)

    def delete_event(self, event_id: str) -> None:
        with self._connect() as connection:
            connection.execute("DELETE FROM events WHERE id = ?", (event_id,))
            connection.execute(
                "DELETE FROM bridges WHERE json_extract(payload, '$.from') = ?",
                (event_id,),
            )
            connection.execute(
                "DELETE FROM bridges WHERE json_extract(payload, '$.to') = ?",
                (event_id,),
            )

    def clear(self) -> None:
        with self._connect() as connection:
            connection.execute("DELETE FROM bridges")
            connection.execute("DELETE FROM events")
            connection.execute("DELETE FROM event_aliases")

    def _init(self) -> None:
        with self._connect() as connection:
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS events (
                    id TEXT PRIMARY KEY,
                    payload TEXT NOT NULL
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS bridges (
                    id TEXT PRIMARY KEY,
                    payload TEXT NOT NULL
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS event_aliases (
                    alias TEXT PRIMARY KEY,
                    canonical_id TEXT NOT NULL
                )
                """
            )

    def _connect(self) -> sqlite3.Connection:
        return sqlite3.connect(self.path)

    def _list_events(self, connection: sqlite3.Connection) -> list[dict[str, Any]]:
        rows = connection.execute("SELECT payload FROM events ORDER BY rowid").fetchall()
        return [json.loads(row[0]) for row in rows]

    def _canonical_event_id(
        self, connection: sqlite3.Connection, payload: dict[str, Any]
    ) -> str:
        event_id = str(payload["id"])
        urls = _event_urls(payload)
        if not urls:
            return event_id

        rows = connection.execute("SELECT id, payload FROM events").fetchall()
        for existing_id, raw_payload in rows:
            if existing_id == event_id:
                return event_id
            existing = json.loads(raw_payload)
            if _same_event_by_source(payload, existing, urls):
                return str(existing_id)
        return event_id

    def _resolve_event_id(
        self, connection: sqlite3.Connection, event_id: str
    ) -> str | None:
        row = connection.execute(
            "SELECT id FROM events WHERE id = ?", (event_id,)
        ).fetchone()
        if row is not None:
            return str(row[0])
        row = connection.execute(
            "SELECT canonical_id FROM event_aliases WHERE alias = ?", (event_id,)
        ).fetchone()
        if row is None:
            return None
        canonical_id = str(row[0])
        exists = connection.execute(
            "SELECT 1 FROM events WHERE id = ?", (canonical_id,)
        ).fetchone()
        return canonical_id if exists is not None else None


def _same_event_by_source(
    incoming: dict[str, Any],
    existing: dict[str, Any],
    incoming_urls: set[str],
) -> bool:
    incoming_primary_url = _primary_url(incoming)
    return bool(incoming_primary_url and incoming_primary_url in _event_urls(existing))


def _event_urls(payload: dict[str, Any]) -> set[str]:
    urls = {_primary_url(payload)}
    for artifact in payload.get("artifacts") or []:
        if isinstance(artifact, dict):
            urls.add(_normalize_url(artifact.get("url")))
    urls.discard("")
    return urls


def _dedupe_artifacts_for_event(
    connection: sqlite3.Connection, payload: dict[str, Any]
) -> dict[str, Any]:
    event_id = str(payload["id"])
    used_urls = _stored_urls_except(connection, event_id)
    primary_url = _primary_url(payload)
    if primary_url:
        used_urls.add(primary_url)

    artifacts: list[dict[str, Any]] = []
    for artifact in payload.get("artifacts") or []:
        if not isinstance(artifact, dict):
            continue
        url = _normalize_url(artifact.get("url"))
        if not url or url in used_urls:
            continue
        used_urls.add(url)
        artifacts.append(artifact)
    return {**payload, "artifacts": artifacts}


def _stored_urls_except(
    connection: sqlite3.Connection, excluded_event_id: str
) -> set[str]:
    rows = connection.execute(
        "SELECT id, payload FROM events WHERE id != ?", (excluded_event_id,)
    ).fetchall()
    urls: set[str] = set()
    for _event_id, raw_payload in rows:
        urls.update(_event_urls(json.loads(raw_payload)))
    return urls


def _primary_url(payload: dict[str, Any]) -> str:
    return _normalize_url(payload.get("url"))


def _normalize_url(value: Any) -> str:
    url = str(value or "").strip()
    if not url:
        return ""
    url = url.split("#", 1)[0].rstrip("/")
    return url.lower()


def _normalize_bridge_label(label: str) -> str:
    return " ".join(label.replace("—", "-").replace("–", "-").split())


def _bridge_key_label(label: str) -> str:
    return _normalize_bridge_label(label).casefold()
