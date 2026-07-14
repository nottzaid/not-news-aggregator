use std::{error::Error, fs, path::Path};

use not_news_domain::{EventId, MoveNode, Point};
use not_news_store::{DurableGraphStore, LegacyGraphReader, StoreError};
use rusqlite::Connection;

const EVENT_A: &str = r#"{"id":"release-a","title":"Release A","date":"Jul 15, 2026","color":4283218390,"summary":"First packaged finding.","sourceLabel":"Primary","artifacts":[{"kind":"link","url":"https://example.test/a","text":"Source A"}]}"#;
const EVENT_B: &str = r#"{"id":"release-b","title":"Release B","date":"Jul 15, 2026","color":4293436492,"summary":"Second packaged finding.","sourceLabel":"Primary","artifacts":[]}"#;
const BRIDGE: &str =
    r#"{"from":"release-a","to":"release-b","label":"Corroborates","provenance":"agent"}"#;

pub struct ReleaseCheck {
    pub database: std::path::PathBuf,
    pub imported_events: usize,
    pub imported_bridges: usize,
    pub final_revision: u64,
}

/// Exercises compatibility and mutation through the exact packaged binary.
/// The caller must provide a path that does not exist; no existing data is
/// opened, replaced, or deleted.
pub fn run(root: &Path) -> Result<ReleaseCheck, Box<dyn Error>> {
    fs::create_dir(root).map_err(|error| {
        format!(
            "release self-check requires a new directory at {}: {error}",
            root.display()
        )
    })?;
    let source = root.join("legacy-source.sqlite");
    create_legacy_source(&source)?;
    let source_before = fs::read(&source)?;
    let legacy = LegacyGraphReader::new(&source).load()?;

    let database = root.join("graph.sqlite");
    let store = DurableGraphStore::open(&database)?;
    let imported = store.import_legacy(&source)?;
    ensure(
        imported.events == legacy.events
            && imported.bridges == legacy.bridges
            && imported.aliases == legacy.aliases
            && imported.placements == legacy.placements,
        "legacy import changed graph knowledge or placement",
    )?;
    ensure(
        fs::read(&source)? == source_before,
        "legacy import modified its source",
    )?;

    let moved = store.commit_move(
        "release-self-check-move",
        &MoveNode {
            event_id: EventId("release-a".into()),
            destination: Point {
                x: 321.5,
                y: -87.25,
            },
            expected_placement_version: 0,
        },
    )?;
    let undone = store
        .undo("release-self-check-undo")?
        .ok_or("release self-check could not undo its move")?;
    ensure(
        undone.snapshot.placements[&EventId("release-a".into())].point
            == Point { x: 10.0, y: 20.0 },
        "undo did not restore the imported placement",
    )?;
    let redone = store
        .redo("release-self-check-redo")?
        .ok_or("release self-check could not redo its move")?;
    ensure(
        redone.snapshot.placements[&EventId("release-a".into())].point
            == Point {
                x: 321.5,
                y: -87.25,
            },
        "redo did not restore the committed placement",
    )?;
    ensure(
        redone.snapshot.revision == moved.snapshot.revision + 2,
        "move/undo/redo revision continuity failed",
    )?;
    let imported_events = imported.events.len();
    let imported_bridges = imported.bridges.len();
    let final_revision = redone.snapshot.revision;
    drop(store);

    let reopened = DurableGraphStore::open(&database)?.load()?;
    ensure(
        reopened == redone.snapshot,
        "reopen changed the committed graph",
    )?;
    ensure(
        fs::read(&source)? == source_before,
        "later mutation modified the legacy source",
    )?;

    reject_future_schema(root)?;
    reject_partial_import(root)?;
    Ok(ReleaseCheck {
        database,
        imported_events,
        imported_bridges,
        final_revision,
    })
}

fn create_legacy_source(path: &Path) -> Result<(), Box<dyn Error>> {
    let connection = Connection::open(path)?;
    connection.execute_batch(
        r"
        CREATE TABLE events (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
        CREATE TABLE bridges (id TEXT PRIMARY KEY, payload TEXT NOT NULL);
        CREATE TABLE event_aliases (alias TEXT PRIMARY KEY, canonical_id TEXT NOT NULL);
        CREATE TABLE placements (
            event_id TEXT PRIMARY KEY, x REAL NOT NULL, y REAL NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE graph_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        ",
    )?;
    connection.execute("INSERT INTO events VALUES ('release-a', ?1)", [EVENT_A])?;
    connection.execute("INSERT INTO events VALUES ('release-b', ?1)", [EVENT_B])?;
    connection.execute(
        "INSERT INTO bridges VALUES ('release-bridge', ?1)",
        [BRIDGE],
    )?;
    connection.execute(
        "INSERT INTO event_aliases VALUES ('release-a-alias', 'release-a')",
        [],
    )?;
    connection.execute(
        "INSERT INTO placements VALUES ('release-a', 10.0, 20.0, 1)",
        [],
    )?;
    connection.execute("INSERT INTO graph_meta VALUES ('revision', '7')", [])?;
    Ok(())
}

fn reject_future_schema(root: &Path) -> Result<(), Box<dyn Error>> {
    let path = root.join("future.sqlite");
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "user_version", 999_i64)?;
    drop(connection);
    let before = fs::read(&path)?;
    ensure(
        matches!(
            DurableGraphStore::open(&path),
            Err(StoreError::UnsupportedSchema(999))
        ),
        "future schema was not rejected",
    )?;
    ensure(
        fs::read(&path)? == before,
        "future-schema rejection modified the database",
    )
}

fn reject_partial_import(root: &Path) -> Result<(), Box<dyn Error>> {
    let malformed = root.join("malformed.sqlite");
    let connection = Connection::open(&malformed)?;
    connection.execute_batch(
        "CREATE TABLE events (id TEXT PRIMARY KEY, payload TEXT NOT NULL);\
         CREATE TABLE bridges (id TEXT PRIMARY KEY, payload TEXT NOT NULL);\
         INSERT INTO events VALUES ('broken', '{');",
    )?;
    drop(connection);
    let destination = root.join("rejected-import.sqlite");
    let store = DurableGraphStore::open(&destination)?;
    ensure(
        store.import_legacy(&malformed).is_err(),
        "malformed legacy import unexpectedly succeeded",
    )?;
    let graph = store.load()?;
    ensure(
        graph.events.is_empty() && graph.revision == 0,
        "failed import left partial durable state",
    )
}

fn ensure(condition: bool, message: &'static str) -> Result<(), Box<dyn Error>> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
