use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use not_news_domain::{EventId, GraphSnapshot, MoveNode, Placement, Point, RestorePlacement};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};

use crate::{StoreError, load_snapshot, table_exists};

const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug)]
pub struct CommitOutcome {
    pub sequence: i64,
    pub snapshot: GraphSnapshot,
}

#[derive(Clone, Debug)]
pub struct DurableGraphStore {
    path: PathBuf,
    migration_backup: Option<PathBuf>,
}

impl DurableGraphStore {
    /// Opens a writable graph and installs the compatible Rust mutation schema.
    /// Existing data is backed up and integrity-checked before the first schema
    /// change; a new empty graph requires no backup.
    ///
    /// # Errors
    ///
    /// Returns an error for inaccessible data, failed backup verification,
    /// unsupported future schemas, or failed transactional migration.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let path = path.into();
        let mut connection = open_connection(&path)?;
        let migration_backup = migrate(&mut connection, &path)?;
        load_snapshot(&connection)?;
        Ok(Self {
            path,
            migration_backup,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn migration_backup(&self) -> Option<&Path> {
        self.migration_backup.as_deref()
    }

    /// Reloads and validates the durable graph.
    ///
    /// # Errors
    ///
    /// Returns an error for inaccessible, malformed, or invariant-breaking data.
    pub fn load(&self) -> Result<GraphSnapshot, StoreError> {
        load_snapshot(&open_connection(&self.path)?)
    }

    /// Commits a placement-only command exactly once for `operation_id`.
    ///
    /// # Errors
    ///
    /// Rejects invalid/stale commands, reused IDs with different payloads,
    /// `SQLite` failures, and state that violates graph invariants.
    pub fn commit_move(
        &self,
        operation_id: &str,
        command: &MoveNode,
    ) -> Result<CommitOutcome, StoreError> {
        validate_operation_id(operation_id)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = mutation_by_operation(&transaction, operation_id)? {
            if existing.kind != MutationKind::Move
                || existing.event != command.event_id
                || existing.expected_version != command.expected_placement_version
                || existing.next.map(|placement| placement.point) != Some(command.destination)
            {
                return Err(StoreError::IdempotencyConflict(operation_id.to_owned()));
            }
            return outcome_and_commit(transaction, existing.sequence);
        }

        let mut graph = load_snapshot(&transaction)?;
        let prior = graph.placements.get(&command.event_id).copied();
        let applied = graph.apply_move(command)?;
        persist_graph_transition(&transaction, &graph, &command.event_id)?;
        let sequence = append_mutation(
            &transaction,
            NewMutation {
                operation_id,
                kind: MutationKind::Move,
                target_sequence: None,
                event: &command.event_id,
                prior,
                next: Some(applied.placement),
                expected_version: command.expected_placement_version,
                committed_version: command.expected_placement_version + 1,
                revision: applied.revision,
            },
        )?;
        transaction.commit()?;
        Ok(CommitOutcome {
            sequence,
            snapshot: graph,
        })
    }

    /// Reverses the latest effective move/redo and appends the inverse to the
    /// immutable mutation log. Returns `None` when nothing is undoable.
    ///
    /// # Errors
    ///
    /// Rejects blank/reused operation IDs, divergent history, invalid graph
    /// state, and failed durable transactions.
    pub fn undo(&self, operation_id: &str) -> Result<Option<CommitOutcome>, StoreError> {
        self.apply_history(operation_id, MutationKind::Undo)
    }

    /// Reapplies the latest effective undo unless a later move cleared its
    /// branch. Returns `None` when nothing is redoable.
    ///
    /// # Errors
    ///
    /// Rejects blank/reused operation IDs, divergent history, invalid graph
    /// state, and failed durable transactions.
    pub fn redo(&self, operation_id: &str) -> Result<Option<CommitOutcome>, StoreError> {
        self.apply_history(operation_id, MutationKind::Redo)
    }

    fn apply_history(
        &self,
        operation_id: &str,
        kind: MutationKind,
    ) -> Result<Option<CommitOutcome>, StoreError> {
        validate_operation_id(operation_id)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = mutation_by_operation(&transaction, operation_id)? {
            if existing.kind != kind {
                return Err(StoreError::IdempotencyConflict(operation_id.to_owned()));
            }
            return outcome_and_commit(transaction, existing.sequence).map(Some);
        }

        let history = replay_history(&transaction)?;
        let target_sequence = match kind {
            MutationKind::Undo => history.undo.last(),
            MutationKind::Redo => history.redo.last(),
            MutationKind::Move => unreachable!("history only applies undo or redo"),
        };
        let Some(&target_sequence) = target_sequence else {
            transaction.commit()?;
            return Ok(None);
        };
        let target = mutation_by_sequence(&transaction, target_sequence)?
            .ok_or(StoreError::HistoryConflict)?;
        let mut graph = load_snapshot(&transaction)?;
        let actual_version = graph
            .placement_versions
            .get(&target.event)
            .copied()
            .unwrap_or_default();
        let current = graph.placements.get(&target.event).copied();
        if actual_version != target.committed_version || current != target.next {
            return Err(StoreError::HistoryConflict);
        }
        let desired = target.prior;
        let revision = graph.restore_placement(&RestorePlacement {
            event_id: target.event.clone(),
            previous: desired,
            expected_placement_version: actual_version,
        })?;
        persist_graph_transition(&transaction, &graph, &target.event)?;
        let sequence = append_mutation(
            &transaction,
            NewMutation {
                operation_id,
                kind,
                target_sequence: Some(target.sequence),
                event: &target.event,
                prior: current,
                next: desired,
                expected_version: actual_version,
                committed_version: actual_version + 1,
                revision,
            },
        )?;
        transaction.commit()?;
        Ok(Some(CommitOutcome {
            sequence,
            snapshot: graph,
        }))
    }
}

fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(2))?;
    Ok(connection)
}

fn migrate(connection: &mut Connection, path: &Path) -> Result<Option<PathBuf>, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema(version));
    }
    if version == SCHEMA_VERSION {
        transaction.commit()?;
        return Ok(None);
    }

    let has_existing_graph = table_exists(&transaction, "events")?;
    if has_existing_graph {
        load_snapshot(&transaction)?;
    }
    let backup = has_existing_graph.then(|| migration_backup_path(path));
    if let Some(backup) = &backup {
        ensure_verified_backup(path, backup)?;
    }
    transaction.execute_batch(
        r"
        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            payload TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS bridges (
            id TEXT PRIMARY KEY,
            payload TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS event_aliases (
            alias TEXT PRIMARY KEY,
            canonical_id TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS placements (
            event_id TEXT PRIMARY KEY,
            x REAL NOT NULL,
            y REAL NOT NULL,
            pinned INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS graph_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO graph_meta (key, value) VALUES ('revision', '0');
        CREATE TABLE placement_versions (
            event_id TEXT PRIMARY KEY,
            version INTEGER NOT NULL CHECK (version >= 0)
        );
        INSERT INTO placement_versions (event_id, version)
            SELECT event_id, 0 FROM placements;
        CREATE TABLE mutation_log (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            operation_id TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN ('move', 'undo', 'redo')),
            target_sequence INTEGER,
            event_id TEXT NOT NULL,
            prior_x REAL,
            prior_y REAL,
            prior_pinned INTEGER,
            next_x REAL,
            next_y REAL,
            next_pinned INTEGER,
            expected_version INTEGER NOT NULL CHECK (expected_version >= 0),
            committed_version INTEGER NOT NULL CHECK (committed_version > expected_version),
            revision INTEGER NOT NULL CHECK (revision >= 0),
            CHECK ((prior_x IS NULL) = (prior_y IS NULL)),
            CHECK ((prior_x IS NULL) = (prior_pinned IS NULL)),
            CHECK ((next_x IS NULL) = (next_y IS NULL)),
            CHECK ((next_x IS NULL) = (next_pinned IS NULL)),
            FOREIGN KEY (target_sequence) REFERENCES mutation_log(sequence)
        );
        PRAGMA user_version = 1;
        ",
    )?;
    transaction.commit()?;
    Ok(backup)
}

fn ensure_verified_backup(source_path: &Path, backup: &Path) -> Result<(), StoreError> {
    if backup.exists() {
        return verify_backup(backup);
    }
    let temporary = temporary_backup_path(backup);
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.backup(rusqlite::MAIN_DB, &temporary, None)?;
    verify_backup(&temporary)?;
    match fs::rename(&temporary, backup) {
        Ok(()) => Ok(()),
        Err(_error) if backup.exists() => {
            fs::remove_file(&temporary)?;
            verify_backup(backup)
        }
        Err(error) => Err(error.into()),
    }
}

fn verify_backup(path: &Path) -> Result<(), StoreError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let result: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(StoreError::InvalidBackup(path.to_owned()))
    }
}

fn migration_backup_path(path: &Path) -> PathBuf {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("pre-rust-v1-{epoch}-{}.sqlite", std::process::id()))
}

fn temporary_backup_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(name)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationKind {
    Move,
    Undo,
    Redo,
}

impl MutationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "move" => Ok(Self::Move),
            "undo" => Ok(Self::Undo),
            "redo" => Ok(Self::Redo),
            _ => Err(StoreError::HistoryConflict),
        }
    }
}

#[derive(Clone, Debug)]
struct Mutation {
    sequence: i64,
    kind: MutationKind,
    _target_sequence: Option<i64>,
    event: EventId,
    prior: Option<Placement>,
    next: Option<Placement>,
    expected_version: u64,
    committed_version: u64,
}

#[derive(Clone, Copy)]
struct NewMutation<'a> {
    operation_id: &'a str,
    kind: MutationKind,
    target_sequence: Option<i64>,
    event: &'a EventId,
    prior: Option<Placement>,
    next: Option<Placement>,
    expected_version: u64,
    committed_version: u64,
    revision: u64,
}

fn mutation_by_operation(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<Mutation>, StoreError> {
    query_mutation(
        connection,
        "SELECT sequence, kind, target_sequence, event_id, prior_x, prior_y, prior_pinned, \
         next_x, next_y, next_pinned, expected_version, committed_version \
         FROM mutation_log WHERE operation_id = ?1",
        rusqlite::params![operation_id],
    )
}

fn mutation_by_sequence(
    connection: &Connection,
    sequence: i64,
) -> Result<Option<Mutation>, StoreError> {
    query_mutation(
        connection,
        "SELECT sequence, kind, target_sequence, event_id, prior_x, prior_y, prior_pinned, \
         next_x, next_y, next_pinned, expected_version, committed_version \
         FROM mutation_log WHERE sequence = ?1",
        rusqlite::params![sequence],
    )
}

fn query_mutation<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    parameters: P,
) -> Result<Option<Mutation>, StoreError> {
    connection
        .query_row(sql, parameters, |row| {
            let kind: String = row.get(1)?;
            let expected: i64 = row.get(10)?;
            let committed: i64 = row.get(11)?;
            Ok((
                row.get(0)?,
                kind,
                row.get(2)?,
                EventId(row.get(3)?),
                placement_from_columns(row, 4)?,
                placement_from_columns(row, 7)?,
                expected,
                committed,
            ))
        })
        .optional()?
        .map(
            |(sequence, kind, target_sequence, event, prior, next, expected, committed)| {
                Ok(Mutation {
                    sequence,
                    kind: MutationKind::parse(&kind)?,
                    _target_sequence: target_sequence,
                    event,
                    prior,
                    next,
                    expected_version: u64::try_from(expected)
                        .map_err(|_| StoreError::HistoryConflict)?,
                    committed_version: u64::try_from(committed)
                        .map_err(|_| StoreError::HistoryConflict)?,
                })
            },
        )
        .transpose()
}

fn placement_from_columns(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> Result<Option<Placement>, rusqlite::Error> {
    let x: Option<f64> = row.get(offset)?;
    let Some(x) = x else {
        return Ok(None);
    };
    Ok(Some(Placement {
        point: Point {
            x,
            y: row.get(offset + 1)?,
        },
        pinned: row.get::<_, i64>(offset + 2)? != 0,
    }))
}

fn append_mutation(connection: &Connection, mutation: NewMutation<'_>) -> Result<i64, StoreError> {
    let (prior_x, prior_y, prior_pinned) = placement_columns(mutation.prior);
    let (next_x, next_y, next_pinned) = placement_columns(mutation.next);
    connection.execute(
        "INSERT INTO mutation_log (operation_id, kind, target_sequence, event_id, \
         prior_x, prior_y, prior_pinned, next_x, next_y, next_pinned, expected_version, \
         committed_version, revision) VALUES \
         (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            mutation.operation_id,
            mutation.kind.as_str(),
            mutation.target_sequence,
            mutation.event.0,
            prior_x,
            prior_y,
            prior_pinned,
            next_x,
            next_y,
            next_pinned,
            to_sql_counter(mutation.expected_version)?,
            to_sql_counter(mutation.committed_version)?,
            to_sql_counter(mutation.revision)?,
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

fn placement_columns(placement: Option<Placement>) -> (Option<f64>, Option<f64>, Option<i64>) {
    placement.map_or((None, None, None), |placement| {
        (
            Some(placement.point.x),
            Some(placement.point.y),
            Some(i64::from(placement.pinned)),
        )
    })
}

fn persist_graph_transition(
    connection: &Connection,
    graph: &GraphSnapshot,
    event: &EventId,
) -> Result<(), StoreError> {
    match graph.placements.get(event) {
        Some(placement) => {
            connection.execute(
                "INSERT INTO placements (event_id, x, y, pinned) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(event_id) DO UPDATE SET x=excluded.x, y=excluded.y, pinned=excluded.pinned",
                params![event.0, placement.point.x, placement.point.y, i64::from(placement.pinned)],
            )?;
        }
        None => {
            connection.execute("DELETE FROM placements WHERE event_id = ?1", [&event.0])?;
        }
    }
    let version = graph
        .placement_versions
        .get(event)
        .copied()
        .ok_or(StoreError::HistoryConflict)?;
    connection.execute(
        "INSERT INTO placement_versions (event_id, version) VALUES (?1, ?2) \
         ON CONFLICT(event_id) DO UPDATE SET version=excluded.version",
        params![event.0, to_sql_counter(version)?],
    )?;
    connection.execute(
        "UPDATE graph_meta SET value = ?1 WHERE key = 'revision'",
        [graph.revision.to_string()],
    )?;
    Ok(())
}

#[derive(Default)]
struct ReplayedHistory {
    undo: Vec<i64>,
    redo: Vec<i64>,
}

fn replay_history(connection: &Connection) -> Result<ReplayedHistory, StoreError> {
    let mut statement = connection
        .prepare("SELECT sequence, kind, target_sequence FROM mutation_log ORDER BY sequence")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })?;
    let mut history = ReplayedHistory::default();
    for row in rows {
        let (sequence, kind, target) = row?;
        match MutationKind::parse(&kind)? {
            MutationKind::Move => {
                history.undo.push(sequence);
                history.redo.clear();
            }
            MutationKind::Undo => {
                if history.undo.pop() != target {
                    return Err(StoreError::HistoryConflict);
                }
                history.redo.push(sequence);
            }
            MutationKind::Redo => {
                if history.redo.pop() != target {
                    return Err(StoreError::HistoryConflict);
                }
                history.undo.push(sequence);
            }
        }
    }
    Ok(history)
}

fn outcome_and_commit(
    transaction: Transaction<'_>,
    sequence: i64,
) -> Result<CommitOutcome, StoreError> {
    let snapshot = load_snapshot(&transaction)?;
    transaction.commit()?;
    Ok(CommitOutcome { sequence, snapshot })
}

fn validate_operation_id(operation_id: &str) -> Result<(), StoreError> {
    if operation_id.trim().is_empty() {
        Err(StoreError::EmptyOperationId)
    } else {
        Ok(())
    }
}

fn to_sql_counter(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::CounterTooLarge(value))
}

#[cfg(test)]
mod tests {
    use not_news_domain::MoveNodeError;
    use tempfile::TempDir;

    use super::*;

    fn legacy_graph() -> (TempDir, PathBuf) {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("graph.sqlite");
        let connection = Connection::open(&path).unwrap();
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
                    '{"id":"a","title":"A","date":"2026-07-14","color":"#112233","summary":"S","sourceLabel":"Source","artifacts":[]}'
                );
                INSERT INTO events VALUES (
                    'b',
                    '{"id":"b","title":"B","date":"2026-07-14","color":"#223344","summary":"S","sourceLabel":"Source","artifacts":[]}'
                );
                INSERT INTO bridges VALUES (
                    'a::b::related',
                    '{"from":"a","to":"b","label":"Related"}'
                );
                INSERT INTO graph_meta VALUES ('revision', '0');
                "##,
            )
            .unwrap();
        drop(connection);
        (directory, path)
    }

    fn command(x: f64, expected_placement_version: u64) -> MoveNode {
        MoveNode {
            event_id: EventId("a".into()),
            destination: Point { x, y: -20.25 },
            expected_placement_version,
        }
    }

    #[test]
    fn migration_makes_a_verified_pre_schema_backup_without_altering_legacy_data() {
        let (_directory, path) = legacy_graph();
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO placements VALUES ('a', 7.25, -8.5, 1)", [])
            .unwrap();
        let before = LegacyFingerprint::read(&path);
        let store = DurableGraphStore::open(&path).unwrap();
        let backup = store.migration_backup().unwrap();

        assert_eq!(LegacyFingerprint::read(backup), before);
        let backup_connection = Connection::open(backup).unwrap();
        assert!(!table_exists(&backup_connection, "mutation_log").unwrap());
        assert_eq!(
            backup_connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let migrated = store.load().unwrap();
        assert_eq!(migrated.events.len(), 2);
        assert_eq!(migrated.bridges.len(), 1);
        assert_eq!(migrated.revision, 0);
    }

    #[test]
    fn move_is_idempotent_and_a_stale_writer_cannot_overwrite_it() {
        let (_directory, path) = legacy_graph();
        let first = DurableGraphStore::open(&path).unwrap();
        let stale = DurableGraphStore::open(&path).unwrap();
        let bridges = first.load().unwrap().bridges;

        let committed = first.commit_move("move-a", &command(10.5, 0)).unwrap();
        assert_eq!(committed.snapshot.revision, 1);
        assert_eq!(committed.snapshot.bridges, bridges);
        assert!(
            (committed.snapshot.placements[&EventId("a".into())].point.x - 10.5).abs()
                < f64::EPSILON
        );
        let duplicate = first.commit_move("move-a", &command(10.5, 0)).unwrap();
        assert_eq!(duplicate.sequence, committed.sequence);
        assert_eq!(duplicate.snapshot.revision, 1);
        assert!(matches!(
            first.commit_move("move-a", &command(11.0, 0)),
            Err(StoreError::IdempotencyConflict(_))
        ));
        assert!(matches!(
            stale.commit_move("stale", &command(99.0, 0)),
            Err(StoreError::Move(MoveNodeError::VersionConflict {
                actual: 1,
                ..
            }))
        ));
        assert_eq!(first.load().unwrap().revision, 1);
    }

    #[test]
    fn undo_and_redo_survive_reopen_as_append_only_transitions() {
        let (_directory, path) = legacy_graph();
        let store = DurableGraphStore::open(&path).unwrap();
        store.commit_move("move", &command(42.0, 0)).unwrap();
        let undone = store.undo("undo").unwrap().unwrap();
        assert!(
            !undone
                .snapshot
                .placements
                .contains_key(&EventId("a".into()))
        );
        assert_eq!(undone.snapshot.placement_versions[&EventId("a".into())], 2);

        drop(store);
        let reopened = DurableGraphStore::open(&path).unwrap();
        let redone = reopened.redo("redo").unwrap().unwrap();
        assert!(
            (redone.snapshot.placements[&EventId("a".into())].point.x - 42.0).abs() < f64::EPSILON
        );
        assert_eq!(redone.snapshot.placement_versions[&EventId("a".into())], 3);
        assert_eq!(redone.snapshot.revision, 3);

        let connection = Connection::open(&path).unwrap();
        let rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM mutation_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 3);
    }

    #[test]
    fn failed_log_append_rolls_back_placement_version_and_revision_together() {
        let (_directory, path) = legacy_graph();
        let store = DurableGraphStore::open(&path).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER inject_log_failure BEFORE INSERT ON mutation_log \
                 BEGIN SELECT RAISE(ABORT, 'injected crash boundary'); END;",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            store.commit_move("must-rollback", &command(55.0, 0)),
            Err(StoreError::Sqlite(_))
        ));
        let after = store.load().unwrap();
        assert!(after.placements.is_empty());
        assert!(after.placement_versions.is_empty());
        assert_eq!(after.revision, 0);
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mutation_log", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn a_new_move_after_undo_preserves_the_log_but_invalidates_redo() {
        let (_directory, path) = legacy_graph();
        let store = DurableGraphStore::open(&path).unwrap();
        store.commit_move("first", &command(10.0, 0)).unwrap();
        store.undo("undo-first").unwrap().unwrap();
        store.commit_move("branch", &command(20.0, 2)).unwrap();

        assert!(store.redo("orphaned-redo").unwrap().is_none());
        let graph = store.load().unwrap();
        assert!((graph.placements[&EventId("a".into())].point.x - 20.0).abs() < f64::EPSILON);
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM mutation_log", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }

    #[test]
    #[ignore = "copies the preserved project databases into temporary migration experiments"]
    fn preserved_databases_migrate_backup_move_and_undo_without_knowledge_drift() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for source in [
            root.join("backend/data/backups/pre-detach-design-20260703.sqlite"),
            root.join("backend/data/graph.sqlite"),
        ] {
            let directory = TempDir::new().unwrap();
            let copy = directory.path().join("graph.sqlite");
            fs::copy(&source, &copy).unwrap();
            let before = crate::LegacyGraphReader::new(&copy).load().unwrap();
            let store = DurableGraphStore::open(&copy).unwrap();
            let migrated = store.load().unwrap();
            assert_knowledge_and_placement_equal(&before, &migrated);
            let backup = crate::LegacyGraphReader::new(store.migration_backup().unwrap())
                .load()
                .unwrap();
            assert_knowledge_and_placement_equal(&before, &backup);

            let event = before.events.keys().next().unwrap().clone();
            let move_command = MoveNode {
                event_id: event,
                destination: Point {
                    x: 1_234_567.25,
                    y: -7_654_321.5,
                },
                expected_placement_version: 0,
            };
            store.commit_move("reference-move", &move_command).unwrap();
            let restored = store.undo("reference-undo").unwrap().unwrap().snapshot;
            assert_knowledge_and_placement_equal(&before, &restored);
            assert_eq!(restored.revision, before.revision + 2);
        }
    }

    fn assert_knowledge_and_placement_equal(expected: &GraphSnapshot, actual: &GraphSnapshot) {
        assert_eq!(actual.events, expected.events);
        assert_eq!(actual.bridges, expected.bridges);
        assert_eq!(actual.aliases, expected.aliases);
        assert_eq!(actual.placements, expected.placements);
    }

    #[derive(Debug, Eq, PartialEq)]
    struct LegacyFingerprint {
        events: i64,
        bridges: i64,
        revision: String,
        placements: Vec<(String, String, String, i64)>,
        integrity: String,
    }

    impl LegacyFingerprint {
        fn read(path: &Path) -> Self {
            let connection =
                Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
            Self {
                events: connection
                    .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
                    .unwrap(),
                bridges: connection
                    .query_row("SELECT COUNT(*) FROM bridges", [], |row| row.get(0))
                    .unwrap(),
                revision: connection
                    .query_row(
                        "SELECT value FROM graph_meta WHERE key='revision'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap(),
                placements: {
                    let mut statement = connection
                        .prepare(
                            "SELECT event_id, printf('%.17g', x), printf('%.17g', y), pinned \
                             FROM placements ORDER BY event_id",
                        )
                        .unwrap();
                    statement
                        .query_map([], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                        })
                        .unwrap()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap()
                },
                integrity: connection
                    .query_row("PRAGMA integrity_check", [], |row| row.get(0))
                    .unwrap(),
            }
        }
    }
}
