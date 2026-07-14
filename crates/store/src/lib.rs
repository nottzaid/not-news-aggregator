mod durable;
mod research;

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use not_news_domain::{
    BridgeId, CurationCommandError, EventBridge, EventId, GraphSnapshot, MoveNodeError, Placement,
    Point, Provenance, ResearchEvent, SnapshotError, SourceArtifact,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use thiserror::Error;

pub use durable::{CommitOutcome, DurableGraphStore};
pub use research::{
    AcceptedResearchMutation, ResearchOutputKind, ResearchSession, ResearchSessionStatus,
};

pub struct LegacyGraphReader {
    path: PathBuf,
}

impl LegacyGraphReader {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Decodes and validates a legacy graph without opening it for writes.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] for inaccessible or invalid `SQLite` data, malformed
    /// JSON payloads, unsupported field values, or graph invariant violations.
    pub fn load(&self) -> Result<GraphSnapshot, StoreError> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        load_snapshot(&connection)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) fn load_snapshot(connection: &Connection) -> Result<GraphSnapshot, StoreError> {
    let events = read_events(connection)?;
    let mut bridges = read_bridges(connection)?;
    bridges
        .retain(|_, bridge| events.contains_key(&bridge.from) && events.contains_key(&bridge.to));
    let mut snapshot = GraphSnapshot {
        events,
        bridges,
        aliases: read_aliases(connection)?,
        ..GraphSnapshot::default()
    };
    if table_exists(connection, "placements")? {
        snapshot.placements = read_placements(connection)?;
        snapshot
            .placements
            .retain(|event, _| snapshot.events.contains_key(event));
    }
    if table_exists(connection, "graph_meta")? {
        snapshot.revision = read_revision(connection)?;
    }
    if table_exists(connection, "placement_versions")? {
        snapshot.placement_versions = read_placement_versions(connection)?;
        snapshot
            .placement_versions
            .retain(|event, _| snapshot.events.contains_key(event));
    }
    snapshot.validate()?;
    Ok(snapshot)
}

fn read_events(connection: &Connection) -> Result<IndexMap<EventId, ResearchEvent>, StoreError> {
    let mut statement = connection.prepare("SELECT payload FROM events ORDER BY rowid")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut events = IndexMap::new();
    for row in rows {
        let payload: Value = serde_json::from_str(&row?)?;
        let event = parse_event(&payload)?;
        if events.insert(event.id.clone(), event).is_some() {
            return Err(StoreError::DuplicateEvent);
        }
    }
    Ok(events)
}

fn read_bridges(connection: &Connection) -> Result<IndexMap<BridgeId, EventBridge>, StoreError> {
    let mut statement = connection.prepare("SELECT id, payload FROM bridges ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut bridges = IndexMap::new();
    for row in rows {
        let (id, payload) = row?;
        let value: Value = serde_json::from_str(&payload)?;
        let bridge = EventBridge {
            id: BridgeId(id),
            from: EventId(required_string(&value, "from")?),
            to: EventId(required_string(&value, "to")?),
            label: required_string(&value, "label")?,
            provenance: parse_provenance(value.get("provenance")),
        };
        if bridges.insert(bridge.id.clone(), bridge).is_some() {
            return Err(StoreError::DuplicateBridge);
        }
    }
    Ok(bridges)
}

fn read_aliases(connection: &Connection) -> Result<IndexMap<EventId, EventId>, StoreError> {
    let mut statement =
        connection.prepare("SELECT alias, canonical_id FROM event_aliases ORDER BY rowid")?;
    let rows = statement.query_map([], |row| Ok((EventId(row.get(0)?), EventId(row.get(1)?))))?;
    let mut aliases = IndexMap::new();
    for row in rows {
        let (alias, canonical) = row?;
        aliases.insert(alias, canonical);
    }
    Ok(aliases)
}

fn read_placements(connection: &Connection) -> Result<IndexMap<EventId, Placement>, StoreError> {
    let mut statement =
        connection.prepare("SELECT event_id, x, y, pinned FROM placements ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((
            EventId(row.get(0)?),
            Placement {
                point: Point {
                    x: row.get(1)?,
                    y: row.get(2)?,
                },
                pinned: row.get::<_, i64>(3)? != 0,
            },
        ))
    })?;
    let mut placements = IndexMap::new();
    for row in rows {
        let (event, placement) = row?;
        placements.insert(event, placement);
    }
    Ok(placements)
}

fn read_revision(connection: &Connection) -> Result<u64, StoreError> {
    let value = connection
        .query_row(
            "SELECT value FROM graph_meta WHERE key = 'revision'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match value {
        None => Ok(0),
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| StoreError::InvalidRevision(value)),
    }
}

fn read_placement_versions(connection: &Connection) -> Result<IndexMap<EventId, u64>, StoreError> {
    let mut statement =
        connection.prepare("SELECT event_id, version FROM placement_versions ORDER BY rowid")?;
    let rows = statement.query_map([], |row| Ok((EventId(row.get(0)?), row.get::<_, i64>(1)?)))?;
    let mut versions = IndexMap::new();
    for row in rows {
        let (event, version) = row?;
        let version = u64::try_from(version)
            .map_err(|_| StoreError::InvalidPlacementVersion(event.clone()))?;
        versions.insert(event, version);
    }
    Ok(versions)
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, rusqlite::Error> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(true),
        )
        .optional()
        .map(Option::unwrap_or_default)
}

fn parse_event(value: &Value) -> Result<ResearchEvent, StoreError> {
    let source_label = required_string(value, "sourceLabel")?;
    let artifacts = value
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or(StoreError::InvalidField("artifacts"))?
        .iter()
        .map(|artifact| parse_artifact(artifact, &source_label))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResearchEvent {
        id: EventId(required_string(value, "id")?),
        title: required_string(value, "title")?,
        date: required_string(value, "date")?,
        color: parse_color(value.get("color"))?,
        summary: required_string(value, "summary")?,
        source_label,
        artifacts,
        url: optional_string(value, "url")?,
    })
}

fn parse_artifact(value: &Value, source_label: &str) -> Result<SourceArtifact, StoreError> {
    if let Some(url) = value.as_str() {
        return Ok(SourceArtifact {
            text: source_label.to_owned(),
            source: source_label.to_owned(),
            url: url.to_owned(),
        });
    }
    Ok(SourceArtifact {
        text: first_string(value, &["text", "label", "title"])
            .unwrap_or_else(|| "Source".to_owned()),
        source: optional_string(value, "source")?.unwrap_or_else(|| source_label.to_owned()),
        url: required_string(value, "url")?,
    })
}

fn parse_color(value: Option<&Value>) -> Result<u32, StoreError> {
    let value = value.ok_or(StoreError::InvalidField("color"))?;
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).map_err(|_| StoreError::InvalidField("color"));
    }
    let text = value
        .as_str()
        .ok_or(StoreError::InvalidField("color"))?
        .trim_start_matches('#');
    let normalized = if text.len() == 6 {
        format!("ff{text}")
    } else {
        text.to_owned()
    };
    u32::from_str_radix(&normalized, 16).map_err(|_| StoreError::InvalidField("color"))
}

fn parse_provenance(value: Option<&Value>) -> Provenance {
    match value.and_then(Value::as_str) {
        Some("agent") => Provenance::Agent,
        Some("user") => Provenance::User,
        _ => Provenance::Legacy,
    }
}

fn required_string(value: &Value, key: &'static str) -> Result<String, StoreError> {
    optional_string(value, key)?.ok_or(StoreError::InvalidField(key))
}

fn optional_string(value: &Value, key: &'static str) -> Result<Option<String>, StoreError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
        Some(_) => Err(StoreError::InvalidField(key)),
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    })
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid JSON payload: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid graph snapshot: {0}")]
    Snapshot(#[from] SnapshotError),
    #[error("invalid or missing field {0}")]
    InvalidField(&'static str),
    #[error("invalid graph revision {0:?}")]
    InvalidRevision(String),
    #[error("invalid placement version for {0:?}")]
    InvalidPlacementVersion(EventId),
    #[error("operation ID must not be blank")]
    EmptyOperationId,
    #[error("legacy import requires a pristine empty destination")]
    ImportDestinationNotEmpty,
    #[error("operation ID {0:?} was already used for a different command")]
    IdempotencyConflict(String),
    #[error("mutation history no longer matches durable placement state")]
    HistoryConflict,
    #[error("database backup failed integrity verification: {0:?}")]
    InvalidBackup(PathBuf),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database schema version {0} is newer than this application supports")]
    UnsupportedSchema(i64),
    #[error("counter {0} cannot be represented by SQLite")]
    CounterTooLarge(u64),
    #[error("research session ID must not be blank")]
    EmptySessionId,
    #[error("research prompt must not be blank")]
    EmptyResearchPrompt,
    #[error("research output message must not be blank")]
    EmptyResearchMessage,
    #[error("research session {0:?} does not exist")]
    MissingResearchSession(String),
    #[error("research session {session:?} is {status}, not running")]
    ResearchSessionClosed { session: String, status: String },
    #[error("research session {session:?} expected output sequence {expected}, received {actual}")]
    ResearchSequenceConflict {
        session: String,
        expected: u64,
        actual: u64,
    },
    #[error("research output {sequence} for session {session:?} was retried with different data")]
    ResearchOutputConflict { session: String, sequence: u64 },
    #[error("research bridge references missing endpoint {0:?}")]
    MissingResearchEndpoint(EventId),
    #[error("research bridge resolves both endpoints to {0:?}")]
    ResearchSelfLoop(EventId),
    #[error("graph revision overflow")]
    RevisionOverflow,
    #[error("graph changed: expected revision {expected}, found {actual}")]
    GraphRevisionConflict { expected: u64, actual: u64 },
    #[error("curation endpoint {0:?} does not exist")]
    MissingCurationEndpoint(EventId),
    #[error("curation relationship {0:?} does not exist")]
    MissingCurationBridge(BridgeId),
    #[error("curation would relate {0:?} to itself")]
    CurationSelfLoop(EventId),
    #[error("curation identity {0:?} is already occupied")]
    CurationIdentityCollision(String),
    #[error("source artifact {0:?} does not exist on the selected event")]
    MissingArtifact(String),
    #[error(transparent)]
    CurationCommand(#[from] CurationCommandError),
    #[error(transparent)]
    Move(#[from] MoveNodeError),
    #[error("duplicate event ID")]
    DuplicateEvent,
    #[error("duplicate bridge ID")]
    DuplicateBridge,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::Connection;
    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn reads_pre_drag_and_drag_era_shapes_without_mutating_them() {
        let file = NamedTempFile::new().unwrap();
        let connection = Connection::open(file.path()).unwrap();
        connection
            .execute_batch(
                r##"
                CREATE TABLE events (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                CREATE TABLE bridges (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
                CREATE TABLE event_aliases (alias TEXT PRIMARY KEY, canonical_id TEXT NOT NULL);
                CREATE TABLE placements (
                    event_id TEXT PRIMARY KEY, x REAL NOT NULL, y REAL NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE graph_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                INSERT INTO events VALUES (
                    'a',
                    '{"id":"a","title":"A","date":"2026-07-14","color":"#112233","summary":"S","sourceLabel":"Source","artifacts":["https://example.com/a"]}'
                );
                INSERT INTO events VALUES (
                    'b',
                    '{"id":"b","title":"B","date":"2026-07-14","color":4279312947,"summary":"S","sourceLabel":"Source","artifacts":[]}'
                );
                INSERT INTO bridges VALUES (
                    'a::b::related',
                    '{"from":"a","to":"b","label":"Related"}'
                );
                INSERT INTO bridges VALUES (
                    'a::missing::related',
                    '{"from":"a","to":"missing","label":"Related"}'
                );
                INSERT INTO event_aliases VALUES ('old-a', 'a');
                INSERT INTO placements VALUES ('a', 10.5, -20.25, 1);
                INSERT INTO placements VALUES ('missing', 90.0, 100.0, 1);
                INSERT INTO graph_meta VALUES ('revision', '4');
                "##,
            )
            .unwrap();
        drop(connection);

        let snapshot = LegacyGraphReader::new(file.path()).load().unwrap();
        assert_eq!(snapshot.events.len(), 2);
        assert_eq!(snapshot.bridges.len(), 1);
        assert_eq!(snapshot.aliases.len(), 1);
        assert_eq!(snapshot.placements.len(), 1);
        assert_eq!(snapshot.revision, 4);
        assert_eq!(snapshot.events[&EventId("a".to_owned())].color, 0xff11_2233);
        assert_eq!(
            snapshot.events[&EventId("a".to_owned())].artifacts[0].text,
            "Source"
        );
    }

    #[test]
    #[ignore = "requires the ignored local reference databases"]
    fn reads_both_preserved_reference_databases() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let backup = LegacyGraphReader::new(
            root.join("backend/data/backups/pre-detach-design-20260703.sqlite"),
        )
        .load()
        .unwrap();
        assert_eq!(
            (
                backup.events.len(),
                backup.bridges.len(),
                backup.aliases.len()
            ),
            (71, 85, 9)
        );
        assert!(backup.placements.is_empty());
        assert_eq!(backup.revision, 0);

        let live = LegacyGraphReader::new(root.join("backend/data/graph.sqlite"))
            .load()
            .unwrap();
        assert_eq!(
            (live.events.len(), live.bridges.len(), live.aliases.len()),
            (71, 81, 9)
        );
        assert_eq!(live.placements.len(), 2);
        assert_eq!(live.revision, 4);
    }
}
