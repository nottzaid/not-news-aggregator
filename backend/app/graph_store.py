from __future__ import annotations

import json
import sqlite3
import uuid
from pathlib import Path
from typing import Any

from .config import PROJECT_ROOT


class GraphRevisionConflict(RuntimeError):
    def __init__(self, expected: int, actual: int) -> None:
        super().__init__(f"Graph revision changed: expected {expected}, found {actual}.")
        self.expected = expected
        self.actual = actual


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

    def snapshot(self) -> dict[str, Any]:
        with self._connect() as connection:
            events = self._list_events(connection)
            event_ids = {event["id"] for event in events}
            bridge_rows = connection.execute(
                "SELECT payload FROM bridges ORDER BY rowid"
            ).fetchall()
            placements = connection.execute(
                "SELECT event_id, x, y, pinned FROM placements"
            ).fetchall()
            revision = self._revision(connection)
        return {
            "events": events,
            "bridges": [
                json.loads(row[0])
                for row in bridge_rows
                if json.loads(row[0]).get("from") in event_ids
                and json.loads(row[0]).get("to") in event_ids
            ],
            "placements": {
                event_id: {"x": x, "y": y, "pinned": bool(pinned)}
                for event_id, x, y, pinned in placements
                if event_id in event_ids
            },
            "revision": revision,
        }

    def create_drag_transaction(
        self,
        *,
        event_id: str,
        origin_x: float,
        origin_y: float,
        destination_x: float,
        destination_y: float,
        target_event_id: str | None,
        expected_revision: int,
    ) -> dict[str, Any]:
        transaction_id = uuid.uuid4().hex
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            revision = self._revision(connection)
            if revision != expected_revision:
                raise GraphRevisionConflict(expected_revision, revision)
            if not self._event_exists(connection, event_id):
                raise KeyError(event_id)
            if target_event_id is not None:
                if target_event_id == event_id or not self._event_exists(
                    connection, target_event_id
                ):
                    raise ValueError("Invalid drag target.")

            old_placement = connection.execute(
                "SELECT x, y, pinned FROM placements WHERE event_id = ?",
                (event_id,),
            ).fetchone()
            old_bridge_rows = connection.execute(
                """
                SELECT id, payload FROM bridges
                WHERE json_extract(payload, '$.from') = ?
                   OR json_extract(payload, '$.to') = ?
                """,
                (event_id, event_id),
            ).fetchall()
            old_bridges = [
                {"id": bridge_id, "payload": json.loads(payload)}
                for bridge_id, payload in old_bridge_rows
            ]

            connection.execute(
                """
                INSERT INTO placements (event_id, x, y, pinned)
                VALUES (?, ?, ?, 1)
                ON CONFLICT(event_id) DO UPDATE SET
                    x = excluded.x, y = excluded.y, pinned = 1
                """,
                (event_id, destination_x, destination_y),
            )

            created_bridge_id = None
            if target_event_id is not None:
                bridge = {
                    "from": event_id,
                    "to": target_event_id,
                    "label": "User-curated relationship",
                    "provenance": "user",
                }
                created_bridge_id = self._upsert_bridge(connection, bridge)

            payload = {
                "id": transaction_id,
                "eventId": event_id,
                "origin": {"x": origin_x, "y": origin_y},
                "destination": {"x": destination_x, "y": destination_y},
                "targetEventId": target_event_id,
                "oldPlacement": (
                    {
                        "x": old_placement[0],
                        "y": old_placement[1],
                        "pinned": bool(old_placement[2]),
                    }
                    if old_placement is not None
                    else None
                ),
                "oldBridges": old_bridges,
                "createdBridgeId": created_bridge_id,
            }
            next_revision = self._bump_revision(connection)
            connection.execute(
                """
                INSERT INTO drag_transactions
                    (id, status, base_revision, committed_revision, payload, plan)
                VALUES (?, 'pending', ?, ?, ?, NULL)
                """,
                (
                    transaction_id,
                    expected_revision,
                    next_revision,
                    json.dumps(payload, separators=(",", ":")),
                ),
            )
        return self.drag_transaction(transaction_id)

    def drag_transaction(self, transaction_id: str) -> dict[str, Any]:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT status, payload, plan FROM drag_transactions WHERE id = ?",
                (transaction_id,),
            ).fetchone()
        if row is None:
            raise KeyError(transaction_id)
        payload = json.loads(row[1])
        return {
            "id": transaction_id,
            "status": row[0],
            **payload,
            "plan": json.loads(row[2]) if row[2] else None,
            "graph": self.snapshot(),
        }

    def reconciliation_context(self, transaction_id: str) -> dict[str, Any]:
        transaction = self.drag_transaction(transaction_id)
        events = {event["id"]: event for event in transaction["graph"]["events"]}
        event_id = transaction["eventId"]
        old_bridges = [
            {"bridgeId": item["id"], **item["payload"]}
            for item in transaction.get("oldBridges", [])
        ]
        neighbor_ids = {
            bridge["to"] if bridge["from"] == event_id else bridge["from"]
            for bridge in old_bridges
        }
        return {
            "transactionId": transaction_id,
            "draggedEvent": events[event_id],
            "oldRelationships": old_bridges,
            "neighbors": [events[node_id] for node_id in neighbor_ids if node_id in events],
            "destinationEvent": events.get(transaction.get("targetEventId")),
            "allowedActions": ["keep", "remove", "amend"],
        }

    def resolve_drag_transaction(
        self,
        transaction_id: str,
        actions: list[dict[str, Any]],
        *,
        fallback: bool = False,
    ) -> dict[str, Any]:
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT status, payload FROM drag_transactions WHERE id = ?",
                (transaction_id,),
            ).fetchone()
            if row is None:
                raise KeyError(transaction_id)
            if row[0] != "pending":
                return self.drag_transaction(transaction_id)
            payload = json.loads(row[1])
            allowed = {item["id"]: item for item in payload["oldBridges"]}
            normalized: list[dict[str, Any]] = []
            seen: set[str] = set()
            for action in actions:
                bridge_id = str(action.get("bridgeId", ""))
                operation = str(action.get("action", ""))
                if bridge_id not in allowed or bridge_id in seen:
                    raise ValueError("Reconciliation referenced an invalid bridge.")
                if operation not in {"keep", "remove", "amend"}:
                    raise ValueError("Invalid reconciliation action.")
                seen.add(bridge_id)
                item = {"bridgeId": bridge_id, "action": operation}
                if operation == "amend":
                    label = _normalize_bridge_label(str(action.get("label", "")))
                    if not label:
                        raise ValueError("Amended relationship requires a label.")
                    item["label"] = label
                item["reason"] = str(action.get("reason", "")).strip()
                normalized.append(item)
            if seen != set(allowed):
                raise ValueError("Reconciliation must decide every old bridge.")

            for action in normalized:
                bridge_id = action["bridgeId"]
                if action["action"] == "remove":
                    connection.execute("DELETE FROM bridges WHERE id = ?", (bridge_id,))
                elif action["action"] == "amend":
                    bridge = dict(allowed[bridge_id]["payload"])
                    bridge["label"] = action["label"]
                    connection.execute("DELETE FROM bridges WHERE id = ?", (bridge_id,))
                    self._upsert_bridge(connection, bridge)
            self._bump_revision(connection)
            status = "fallback" if fallback else "resolved"
            connection.execute(
                "UPDATE drag_transactions SET status = ?, plan = ? WHERE id = ?",
                (
                    status,
                    json.dumps(normalized, separators=(",", ":")),
                    transaction_id,
                ),
            )
        return self.drag_transaction(transaction_id)

    def fallback_drag_transaction(self, transaction_id: str) -> dict[str, Any]:
        transaction = self.drag_transaction(transaction_id)
        actions = [
            {
                "bridgeId": item["id"],
                "action": "remove",
                "reason": "Deterministic fallback after reconciliation failure.",
            }
            for item in transaction.get("oldBridges", [])
        ]
        return self.resolve_drag_transaction(
            transaction_id, actions, fallback=True
        )

    def undo_drag_transaction(self, transaction_id: str) -> dict[str, Any]:
        with self._connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            row = connection.execute(
                "SELECT status, payload FROM drag_transactions WHERE id = ?",
                (transaction_id,),
            ).fetchone()
            if row is None:
                raise KeyError(transaction_id)
            if row[0] == "undone":
                return self.drag_transaction(transaction_id)
            payload = json.loads(row[1])
            event_id = payload["eventId"]
            connection.execute(
                """
                DELETE FROM bridges
                WHERE json_extract(payload, '$.from') = ?
                   OR json_extract(payload, '$.to') = ?
                """,
                (event_id, event_id),
            )
            for item in payload["oldBridges"]:
                connection.execute(
                    "INSERT OR REPLACE INTO bridges (id, payload) VALUES (?, ?)",
                    (
                        item["id"],
                        json.dumps(item["payload"], separators=(",", ":")),
                    ),
                )
            old_placement = payload.get("oldPlacement")
            if old_placement is None:
                connection.execute(
                    "DELETE FROM placements WHERE event_id = ?", (event_id,)
                )
            else:
                connection.execute(
                    """
                    INSERT OR REPLACE INTO placements (event_id, x, y, pinned)
                    VALUES (?, ?, ?, ?)
                    """,
                    (
                        event_id,
                        old_placement["x"],
                        old_placement["y"],
                        int(old_placement["pinned"]),
                    ),
                )
            self._bump_revision(connection)
            connection.execute(
                "UPDATE drag_transactions SET status = 'undone' WHERE id = ?",
                (transaction_id,),
            )
        return self.drag_transaction(transaction_id)

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
            connection.execute("DELETE FROM placements WHERE event_id = ?", (event_id,))

    def clear(self) -> None:
        with self._connect() as connection:
            connection.execute("DELETE FROM bridges")
            connection.execute("DELETE FROM events")
            connection.execute("DELETE FROM event_aliases")
            connection.execute("DELETE FROM placements")
            connection.execute("DELETE FROM drag_transactions")
            connection.execute(
                "UPDATE graph_meta SET value = '0' WHERE key = 'revision'"
            )

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
                CREATE TABLE IF NOT EXISTS placements (
                    event_id TEXT PRIMARY KEY,
                    x REAL NOT NULL,
                    y REAL NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 1
                )
                """
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS graph_meta (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                )
                """
            )
            connection.execute(
                "INSERT OR IGNORE INTO graph_meta (key, value) VALUES ('revision', '0')"
            )
            connection.execute(
                """
                CREATE TABLE IF NOT EXISTS drag_transactions (
                    id TEXT PRIMARY KEY,
                    status TEXT NOT NULL,
                    base_revision INTEGER NOT NULL,
                    committed_revision INTEGER NOT NULL,
                    payload TEXT NOT NULL,
                    plan TEXT
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

    def _event_exists(self, connection: sqlite3.Connection, event_id: str) -> bool:
        return (
            connection.execute(
                "SELECT 1 FROM events WHERE id = ?", (event_id,)
            ).fetchone()
            is not None
        )

    def _revision(self, connection: sqlite3.Connection) -> int:
        row = connection.execute(
            "SELECT value FROM graph_meta WHERE key = 'revision'"
        ).fetchone()
        return int(row[0]) if row else 0

    def _bump_revision(self, connection: sqlite3.Connection) -> int:
        revision = self._revision(connection) + 1
        connection.execute(
            "UPDATE graph_meta SET value = ? WHERE key = 'revision'",
            (str(revision),),
        )
        return revision

    def _upsert_bridge(
        self, connection: sqlite3.Connection, payload: dict[str, Any]
    ) -> str:
        from_id = self._resolve_event_id(connection, payload["from"])
        to_id = self._resolve_event_id(connection, payload["to"])
        if from_id is None or to_id is None or from_id == to_id:
            raise ValueError("Bridge endpoints must be distinct existing events.")
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
        return key

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
