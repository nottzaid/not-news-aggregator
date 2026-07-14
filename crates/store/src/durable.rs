use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use not_news_domain::{
    BridgeId, DetachRelationship, EventBridge, EventId, GraphSnapshot, MoveNode, Placement, Point,
    PromoteArtifact, Provenance, RelateEvents, ResearchEvent, RestorePlacement, SourceArtifact,
    normalize_predicate,
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};

use crate::research::normalize_url;
use crate::{StoreError, load_snapshot, table_exists};

const SCHEMA_VERSION: i64 = 4;

#[derive(Clone, Debug)]
pub struct CommitOutcome {
    pub sequence: i64,
    pub snapshot: GraphSnapshot,
}

#[derive(Clone, Debug)]
pub struct DurableGraphStore {
    pub(crate) path: PathBuf,
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
                || existing.event.as_ref() != Some(&command.event_id)
                || existing.expected_version != Some(command.expected_placement_version)
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

    /// Creates one explicitly identified semantic relationship.
    ///
    /// # Errors
    ///
    /// Rejects stale revisions, missing or identical endpoints, invalid
    /// predicates, identity collisions, contradictory retries, and failed
    /// transactions without changing the graph.
    pub fn commit_relation(
        &self,
        operation_id: &str,
        command: &RelateEvents,
    ) -> Result<CommitOutcome, StoreError> {
        validate_operation_id(operation_id)?;
        let request = serde_json::to_string(command)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = mutation_by_operation(&transaction, operation_id)? {
            return retry_semantic(
                transaction,
                &existing,
                operation_id,
                MutationKind::Relate,
                &request,
            );
        }
        let graph = load_snapshot(&transaction)?;
        require_revision(&graph, command.expected_revision)?;
        let from = resolve_graph_event(&graph, &command.from)?;
        let to = resolve_graph_event(&graph, &command.to)?;
        if from == to {
            return Err(StoreError::CurationSelfLoop(from));
        }
        if graph.bridges.contains_key(&command.bridge_id) {
            return Err(StoreError::CurationIdentityCollision(
                command.bridge_id.0.clone(),
            ));
        }
        let bridge = EventBridge {
            id: command.bridge_id.clone(),
            from,
            to,
            label: normalize_predicate(&command.predicate)?,
            provenance: command.provenance,
        };
        let transition = SemanticTransition {
            prior: SemanticState::bridge(command.bridge_id.clone(), None),
            next: SemanticState::bridge(command.bridge_id.clone(), Some(bridge)),
        };
        commit_semantic_transition(
            transaction,
            operation_id,
            MutationKind::Relate,
            &request,
            graph,
            &transition,
        )
    }

    /// Removes exactly one explicitly identified relationship.
    ///
    /// # Errors
    ///
    /// Rejects stale revisions, absent bridges, contradictory retries, and
    /// failed transactions without inferring any proximity-based scope.
    pub fn commit_detachment(
        &self,
        operation_id: &str,
        command: &DetachRelationship,
    ) -> Result<CommitOutcome, StoreError> {
        validate_operation_id(operation_id)?;
        let request = serde_json::to_string(command)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = mutation_by_operation(&transaction, operation_id)? {
            return retry_semantic(
                transaction,
                &existing,
                operation_id,
                MutationKind::Detach,
                &request,
            );
        }
        let graph = load_snapshot(&transaction)?;
        require_revision(&graph, command.expected_revision)?;
        let bridge = graph
            .bridges
            .get(&command.bridge_id)
            .cloned()
            .ok_or_else(|| StoreError::MissingCurationBridge(command.bridge_id.clone()))?;
        let transition = SemanticTransition {
            prior: SemanticState::bridge(command.bridge_id.clone(), Some(bridge)),
            next: SemanticState::bridge(command.bridge_id.clone(), None),
        };
        commit_semantic_transition(
            transaction,
            operation_id,
            MutationKind::Detach,
            &request,
            graph,
            &transition,
        )
    }

    /// Promotes one named artifact into a first-class event or an alias to an
    /// existing event with the same canonical primary URL. An optional explicit
    /// relationship commits in the same transaction.
    ///
    /// # Errors
    ///
    /// Rejects stale revisions, missing artifacts, identity collisions,
    /// self-relations, contradictory retries, invalid predicates, and partial
    /// persistence.
    pub fn commit_artifact_promotion(
        &self,
        operation_id: &str,
        command: &PromoteArtifact,
    ) -> Result<CommitOutcome, StoreError> {
        validate_operation_id(operation_id)?;
        let request = serde_json::to_string(command)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = mutation_by_operation(&transaction, operation_id)? {
            return retry_semantic(
                transaction,
                &existing,
                operation_id,
                MutationKind::Promote,
                &request,
            );
        }
        let graph = load_snapshot(&transaction)?;
        require_revision(&graph, command.expected_revision)?;
        let source_id = resolve_graph_event(&graph, &command.source_event)?;
        let source = &graph.events[&source_id];
        let artifact_key = normalize_url(&command.artifact_url);
        let artifact = source
            .artifacts
            .iter()
            .find(|artifact| normalize_url(&artifact.url) == artifact_key)
            .cloned()
            .ok_or_else(|| StoreError::MissingArtifact(command.artifact_url.clone()))?;
        if graph.events.contains_key(&command.promoted_id)
            || graph.aliases.contains_key(&command.promoted_id)
        {
            return Err(StoreError::CurationIdentityCollision(
                command.promoted_id.0.clone(),
            ));
        }

        let canonical = graph
            .events
            .values()
            .find(|event| {
                event
                    .url
                    .as_deref()
                    .is_some_and(|url| normalize_url(url) == artifact_key)
            })
            .map(|event| event.id.clone());
        let mut prior = SemanticState::default();
        let mut next = SemanticState::default();
        let canonical = if let Some(canonical) = canonical {
            prior.aliases.push((command.promoted_id.clone(), None));
            next.aliases
                .push((command.promoted_id.clone(), Some(canonical.clone())));
            canonical
        } else {
            let promoted = promoted_event(command, source, &artifact)?;
            prior.events.push((promoted.id.clone(), None));
            next.events
                .push((promoted.id.clone(), Some(promoted.clone())));
            promoted.id
        };

        if let Some(relation) = &command.relation {
            if source_id == canonical {
                return Err(StoreError::CurationSelfLoop(source_id));
            }
            if graph.bridges.contains_key(&relation.bridge_id) {
                return Err(StoreError::CurationIdentityCollision(
                    relation.bridge_id.0.clone(),
                ));
            }
            let bridge = EventBridge {
                id: relation.bridge_id.clone(),
                from: source_id,
                to: canonical,
                label: normalize_predicate(&relation.predicate)?,
                provenance: Provenance::User,
            };
            prior.bridges.push((relation.bridge_id.clone(), None));
            next.bridges
                .push((relation.bridge_id.clone(), Some(bridge)));
        }
        commit_semantic_transition(
            transaction,
            operation_id,
            MutationKind::Promote,
            &request,
            graph,
            &SemanticTransition { prior, next },
        )
    }

    /// Reverses the latest effective graph command/redo and appends the inverse to the
    /// immutable mutation log. Returns `None` when nothing is undoable.
    ///
    /// # Errors
    ///
    /// Rejects blank/reused operation IDs, divergent history, invalid graph
    /// state, and failed durable transactions.
    pub fn undo(&self, operation_id: &str) -> Result<Option<CommitOutcome>, StoreError> {
        self.apply_history(operation_id, MutationKind::Undo)
    }

    /// Reapplies the latest effective undo unless a later command cleared its
    /// branch. Returns `None` when nothing is redoable.
    ///
    /// # Errors
    ///
    /// Rejects blank/reused operation IDs, divergent history, invalid graph
    /// state, and failed durable transactions.
    pub fn redo(&self, operation_id: &str) -> Result<Option<CommitOutcome>, StoreError> {
        self.apply_history(operation_id, MutationKind::Redo)
    }

    /// Atomically erases the canvas, placement history, and research trail.
    /// The graph revision remains monotonic and the operation is retry-safe.
    ///
    /// # Errors
    ///
    /// Rejects blank or conflicting operation IDs, revision overflow, invalid
    /// durable state, and any transaction failure without partially clearing.
    pub fn clear(&self, operation_id: &str) -> Result<GraphSnapshot, StoreError> {
        validate_operation_id(operation_id)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(committed_revision) = transaction
            .query_row(
                "SELECT committed_revision FROM destructive_log \
                 WHERE operation_id=?1 AND kind='clear'",
                [operation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            let graph = load_snapshot(&transaction)?;
            if to_sql_counter(graph.revision)? != committed_revision {
                return Err(StoreError::IdempotencyConflict(operation_id.to_owned()));
            }
            transaction.commit()?;
            return Ok(graph);
        }

        let graph = load_snapshot(&transaction)?;
        let revision = graph
            .revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        transaction.execute_batch(
            r"
            DELETE FROM research_output_log;
            DELETE FROM research_sessions;
            DELETE FROM mutation_log;
            DELETE FROM placements;
            DELETE FROM placement_versions;
            DELETE FROM bridges;
            DELETE FROM event_aliases;
            DELETE FROM events;
            ",
        )?;
        transaction.execute(
            "UPDATE graph_meta SET value=?1 WHERE key='revision'",
            [revision.to_string()],
        )?;
        transaction.execute(
            "INSERT INTO destructive_log \
             (operation_id, kind, committed_revision) VALUES (?1, 'clear', ?2)",
            params![operation_id, to_sql_counter(revision)?],
        )?;
        let cleared = load_snapshot(&transaction)?;
        transaction.commit()?;
        Ok(cleared)
    }

    /// Imports one validated legacy graph into a pristine Rust database while
    /// opening the source read-only.
    ///
    /// # Errors
    ///
    /// Rejects malformed sources and any destination containing graph,
    /// research, history, or destructive state; failed insertion is atomic.
    pub fn import_legacy(&self, source: &Path) -> Result<GraphSnapshot, StoreError> {
        let mut imported = crate::LegacyGraphReader::new(source).load()?;
        for event in imported.placements.keys() {
            imported
                .placement_versions
                .entry(event.clone())
                .or_insert(0);
        }
        imported.validate()?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_snapshot(&transaction)?;
        let hidden_rows: i64 = transaction.query_row(
            "SELECT \
                (SELECT count(*) FROM mutation_log) + \
                (SELECT count(*) FROM research_sessions) + \
                (SELECT count(*) FROM research_output_log) + \
                (SELECT count(*) FROM destructive_log)",
            [],
            |row| row.get(0),
        )?;
        if current != GraphSnapshot::default() || hidden_rows != 0 {
            return Err(StoreError::ImportDestinationNotEmpty);
        }

        for (id, event) in &imported.events {
            transaction.execute(
                "INSERT INTO events (id, payload) VALUES (?1, ?2)",
                params![id.0, serde_json::to_string(event)?],
            )?;
        }
        for (id, bridge) in &imported.bridges {
            let payload = serde_json::to_string(&serde_json::json!({
                "from": bridge.from,
                "to": bridge.to,
                "label": bridge.label,
                "provenance": bridge.provenance,
            }))?;
            transaction.execute(
                "INSERT INTO bridges (id, payload) VALUES (?1, ?2)",
                params![id.0, payload],
            )?;
        }
        for (alias, canonical) in &imported.aliases {
            transaction.execute(
                "INSERT INTO event_aliases (alias, canonical_id) VALUES (?1, ?2)",
                params![alias.0, canonical.0],
            )?;
        }
        for (event, placement) in &imported.placements {
            transaction.execute(
                "INSERT INTO placements (event_id, x, y, pinned) VALUES (?1, ?2, ?3, ?4)",
                params![
                    event.0,
                    placement.point.x,
                    placement.point.y,
                    i64::from(placement.pinned)
                ],
            )?;
            let version = imported
                .placement_versions
                .get(event)
                .copied()
                .unwrap_or_default();
            transaction.execute(
                "INSERT INTO placement_versions (event_id, version) VALUES (?1, ?2)",
                params![event.0, to_sql_counter(version)?],
            )?;
        }
        transaction.execute(
            "UPDATE graph_meta SET value=?1 WHERE key='revision'",
            [imported.revision.to_string()],
        )?;
        let snapshot = load_snapshot(&transaction)?;
        if snapshot != imported {
            return Err(StoreError::HistoryConflict);
        }
        transaction.commit()?;
        Ok(snapshot)
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
            _ => unreachable!("history only applies undo or redo"),
        };
        let Some(&target_sequence) = target_sequence else {
            transaction.commit()?;
            return Ok(None);
        };
        let target = mutation_by_sequence(&transaction, target_sequence)?
            .ok_or(StoreError::HistoryConflict)?;
        let mut graph = load_snapshot(&transaction)?;
        if target.revision > graph.revision {
            return Err(StoreError::HistoryConflict);
        }
        let sequence = if let Some(transition) = target.semantic.as_ref() {
            let inverse = transition.reversed();
            let revision = inverse.apply(&mut graph)?;
            persist_semantic_transition(&transaction, &graph, &inverse.next)?;
            append_semantic_mutation(
                &transaction,
                NewSemanticMutation {
                    operation_id,
                    kind,
                    target_sequence: Some(target.sequence),
                    request_json: None,
                    transition: &inverse,
                    revision,
                },
            )?
        } else {
            let event = target.event.as_ref().ok_or(StoreError::HistoryConflict)?;
            let expected_version = target
                .committed_version
                .ok_or(StoreError::HistoryConflict)?;
            let actual_version = graph
                .placement_versions
                .get(event)
                .copied()
                .unwrap_or_default();
            let current = graph.placements.get(event).copied();
            if actual_version != expected_version || current != target.next {
                return Err(StoreError::HistoryConflict);
            }
            let desired = target.prior;
            let revision = graph.restore_placement(&RestorePlacement {
                event_id: event.clone(),
                previous: desired,
                expected_placement_version: actual_version,
            })?;
            persist_graph_transition(&transaction, &graph, event)?;
            append_mutation(
                &transaction,
                NewMutation {
                    operation_id,
                    kind,
                    target_sequence: Some(target.sequence),
                    event,
                    prior: current,
                    next: desired,
                    expected_version: actual_version,
                    committed_version: actual_version + 1,
                    revision,
                },
            )?
        };
        transaction.commit()?;
        Ok(Some(CommitOutcome {
            sequence,
            snapshot: graph,
        }))
    }
}

pub(crate) fn open_connection(path: &Path) -> Result<Connection, StoreError> {
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
    let backup = has_existing_graph.then(|| migration_backup_path(path, version));
    if let Some(backup) = &backup {
        ensure_verified_backup(path, backup)?;
    }
    if version == 0 {
        install_schema_v1(&transaction)?;
    }
    if version <= 1 {
        install_schema_v2(&transaction)?;
    }
    if version <= 2 {
        install_schema_v3(&transaction)?;
    }
    if version <= 3 {
        install_schema_v4(&transaction)?;
    }
    transaction.commit()?;
    Ok(backup)
}

fn install_schema_v1(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
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
    Ok(())
}

fn install_schema_v2(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        r"
        CREATE TABLE research_sessions (
            id TEXT PRIMARY KEY,
            prompt TEXT NOT NULL CHECK (length(trim(prompt)) > 0),
            status TEXT NOT NULL CHECK (status IN ('running', 'done', 'error', 'interrupted')),
            last_output_sequence INTEGER,
            message TEXT,
            started_at INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
            CHECK (last_output_sequence IS NULL OR last_output_sequence >= 0)
        );
        CREATE TABLE research_output_log (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            output_sequence INTEGER NOT NULL CHECK (output_sequence >= 0),
            kind TEXT NOT NULL CHECK (kind IN (
                'message', 'voice_note', 'event', 'bridge', 'done', 'error', 'protocol_error'
            )),
            payload TEXT NOT NULL,
            canonical_key TEXT,
            graph_revision INTEGER NOT NULL CHECK (graph_revision >= 0),
            created_at INTEGER NOT NULL DEFAULT (unixepoch()),
            UNIQUE (session_id, output_sequence),
            FOREIGN KEY (session_id) REFERENCES research_sessions(id)
        );
        PRAGMA user_version = 2;
        ",
    )?;
    Ok(())
}

fn install_schema_v3(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        r"
        CREATE TABLE destructive_log (
            operation_id TEXT PRIMARY KEY,
            kind TEXT NOT NULL CHECK (kind = 'clear'),
            committed_revision INTEGER NOT NULL CHECK (committed_revision >= 0),
            committed_at INTEGER NOT NULL DEFAULT (unixepoch())
        );
        PRAGMA user_version = 3;
        ",
    )?;
    Ok(())
}

fn install_schema_v4(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        r"
        ALTER TABLE mutation_log RENAME TO mutation_log_v3;
        CREATE TABLE mutation_log (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            operation_id TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN (
                'move', 'relate', 'detach', 'promote', 'undo', 'redo'
            )),
            target_sequence INTEGER,
            event_id TEXT,
            prior_x REAL,
            prior_y REAL,
            prior_pinned INTEGER,
            next_x REAL,
            next_y REAL,
            next_pinned INTEGER,
            expected_version INTEGER CHECK (expected_version >= 0),
            committed_version INTEGER,
            revision INTEGER NOT NULL CHECK (revision >= 0),
            request_json TEXT,
            prior_json TEXT,
            next_json TEXT,
            CHECK ((prior_x IS NULL) = (prior_y IS NULL)),
            CHECK ((prior_x IS NULL) = (prior_pinned IS NULL)),
            CHECK ((next_x IS NULL) = (next_y IS NULL)),
            CHECK ((next_x IS NULL) = (next_pinned IS NULL)),
            CHECK (
                (event_id IS NOT NULL AND expected_version IS NOT NULL AND
                 committed_version > expected_version AND prior_json IS NULL AND
                 next_json IS NULL)
                OR
                (event_id IS NULL AND expected_version IS NULL AND
                 committed_version IS NULL AND prior_json IS NOT NULL AND
                 next_json IS NOT NULL)
            ),
            FOREIGN KEY (target_sequence) REFERENCES mutation_log(sequence)
        );
        INSERT INTO mutation_log (
            sequence, operation_id, kind, target_sequence, event_id,
            prior_x, prior_y, prior_pinned, next_x, next_y, next_pinned,
            expected_version, committed_version, revision
        )
        SELECT sequence, operation_id, kind, target_sequence, event_id,
            prior_x, prior_y, prior_pinned, next_x, next_y, next_pinned,
            expected_version, committed_version, revision
        FROM mutation_log_v3 ORDER BY sequence;
        DROP TABLE mutation_log_v3;
        PRAGMA user_version = 4;
        ",
    )?;
    Ok(())
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

fn migration_backup_path(path: &Path, version: i64) -> PathBuf {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!(
        "pre-rust-v{}-{epoch}-{}.sqlite",
        version + 1,
        std::process::id()
    ))
}

fn temporary_backup_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(name)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationKind {
    Move,
    Relate,
    Detach,
    Promote,
    Undo,
    Redo,
}

impl MutationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Relate => "relate",
            Self::Detach => "detach",
            Self::Promote => "promote",
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "move" => Ok(Self::Move),
            "relate" => Ok(Self::Relate),
            "detach" => Ok(Self::Detach),
            "promote" => Ok(Self::Promote),
            "undo" => Ok(Self::Undo),
            "redo" => Ok(Self::Redo),
            _ => Err(StoreError::HistoryConflict),
        }
    }

    fn is_action(self) -> bool {
        matches!(
            self,
            Self::Move | Self::Relate | Self::Detach | Self::Promote
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticState {
    events: Vec<(EventId, Option<ResearchEvent>)>,
    bridges: Vec<(BridgeId, Option<EventBridge>)>,
    aliases: Vec<(EventId, Option<EventId>)>,
}

impl SemanticState {
    fn bridge(id: BridgeId, value: Option<EventBridge>) -> Self {
        Self {
            bridges: vec![(id, value)],
            ..Self::default()
        }
    }

    fn matches(&self, graph: &GraphSnapshot) -> bool {
        self.events
            .iter()
            .all(|(id, value)| graph.events.get(id) == value.as_ref())
            && self
                .bridges
                .iter()
                .all(|(id, value)| graph.bridges.get(id) == value.as_ref())
            && self
                .aliases
                .iter()
                .all(|(id, value)| graph.aliases.get(id) == value.as_ref())
    }

    fn apply(&self, graph: &mut GraphSnapshot) {
        for (id, value) in &self.events {
            set_entry(&mut graph.events, id, value.as_ref());
            if value.is_none() {
                graph.placements.shift_remove(id);
                graph.placement_versions.shift_remove(id);
            }
        }
        for (id, value) in &self.bridges {
            set_entry(&mut graph.bridges, id, value.as_ref());
        }
        for (id, value) in &self.aliases {
            set_entry(&mut graph.aliases, id, value.as_ref());
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SemanticTransition {
    prior: SemanticState,
    next: SemanticState,
}

impl SemanticTransition {
    fn reversed(&self) -> Self {
        Self {
            prior: self.next.clone(),
            next: self.prior.clone(),
        }
    }

    fn apply(&self, graph: &mut GraphSnapshot) -> Result<u64, StoreError> {
        if !self.prior.matches(graph) {
            return Err(StoreError::HistoryConflict);
        }
        let mut candidate = graph.clone();
        self.next.apply(&mut candidate);
        candidate.revision = candidate
            .revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        candidate.validate()?;
        let revision = candidate.revision;
        *graph = candidate;
        Ok(revision)
    }
}

fn set_entry<K, V>(map: &mut indexmap::IndexMap<K, V>, key: &K, value: Option<&V>)
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
{
    match value {
        Some(value) => {
            map.insert(key.clone(), value.clone());
        }
        None => {
            map.shift_remove(key);
        }
    }
}

#[derive(Clone, Debug)]
struct Mutation {
    sequence: i64,
    kind: MutationKind,
    _target_sequence: Option<i64>,
    event: Option<EventId>,
    prior: Option<Placement>,
    next: Option<Placement>,
    expected_version: Option<u64>,
    committed_version: Option<u64>,
    revision: u64,
    request_json: Option<String>,
    semantic: Option<SemanticTransition>,
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
         next_x, next_y, next_pinned, expected_version, committed_version, request_json, \
         prior_json, next_json, revision \
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
         next_x, next_y, next_pinned, expected_version, committed_version, request_json, \
         prior_json, next_json, revision \
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
            Ok((
                row.get(0)?,
                kind,
                row.get(2)?,
                row.get::<_, Option<String>>(3)?.map(EventId),
                placement_from_columns(row, 4)?,
                placement_from_columns(row, 7)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, i64>(15)?,
            ))
        })
        .optional()?
        .map(
            |(
                sequence,
                kind,
                target_sequence,
                event,
                prior,
                next,
                expected,
                committed,
                request_json,
                prior_json,
                next_json,
                revision,
            )| {
                let semantic = match (prior_json, next_json) {
                    (Some(prior), Some(next)) => Some(SemanticTransition {
                        prior: serde_json::from_str(&prior)?,
                        next: serde_json::from_str(&next)?,
                    }),
                    (None, None) => None,
                    _ => return Err(StoreError::HistoryConflict),
                };
                Ok(Mutation {
                    sequence,
                    kind: MutationKind::parse(&kind)?,
                    _target_sequence: target_sequence,
                    event,
                    prior,
                    next,
                    expected_version: expected
                        .map(|value| u64::try_from(value).map_err(|_| StoreError::HistoryConflict))
                        .transpose()?,
                    committed_version: committed
                        .map(|value| u64::try_from(value).map_err(|_| StoreError::HistoryConflict))
                        .transpose()?,
                    revision: u64::try_from(revision).map_err(|_| StoreError::HistoryConflict)?,
                    request_json,
                    semantic,
                })
            },
        )
        .transpose()
}

fn retry_semantic(
    transaction: Transaction<'_>,
    existing: &Mutation,
    operation_id: &str,
    expected_kind: MutationKind,
    request: &str,
) -> Result<CommitOutcome, StoreError> {
    if existing.kind != expected_kind
        || existing.request_json.as_deref() != Some(request)
        || existing.semantic.is_none()
    {
        return Err(StoreError::IdempotencyConflict(operation_id.to_owned()));
    }
    outcome_and_commit(transaction, existing.sequence)
}

fn require_revision(graph: &GraphSnapshot, expected: u64) -> Result<(), StoreError> {
    if graph.revision == expected {
        Ok(())
    } else {
        Err(StoreError::GraphRevisionConflict {
            expected,
            actual: graph.revision,
        })
    }
}

fn resolve_graph_event(graph: &GraphSnapshot, id: &EventId) -> Result<EventId, StoreError> {
    if graph.events.contains_key(id) {
        return Ok(id.clone());
    }
    graph
        .aliases
        .get(id)
        .filter(|canonical| graph.events.contains_key(*canonical))
        .cloned()
        .ok_or_else(|| StoreError::MissingCurationEndpoint(id.clone()))
}

fn promoted_event(
    command: &PromoteArtifact,
    source: &ResearchEvent,
    artifact: &SourceArtifact,
) -> Result<ResearchEvent, StoreError> {
    let event = ResearchEvent {
        id: command.promoted_id.clone(),
        title: artifact.text.trim().to_owned(),
        date: command.date.trim().to_owned(),
        color: source.color,
        summary: format!("Promoted evidence from {}.", source.title.trim()),
        source_label: artifact.source.trim().to_owned(),
        artifacts: Vec::new(),
        url: Some(artifact.url.trim().to_owned()),
    };
    let mut candidate = GraphSnapshot::default();
    candidate.events.insert(event.id.clone(), event.clone());
    candidate.validate()?;
    Ok(event)
}

fn commit_semantic_transition(
    transaction: Transaction<'_>,
    operation_id: &str,
    kind: MutationKind,
    request: &str,
    mut graph: GraphSnapshot,
    transition: &SemanticTransition,
) -> Result<CommitOutcome, StoreError> {
    if !kind.is_action() || kind == MutationKind::Move {
        return Err(StoreError::HistoryConflict);
    }
    let revision = transition.apply(&mut graph)?;
    persist_semantic_transition(&transaction, &graph, &transition.next)?;
    let sequence = append_semantic_mutation(
        &transaction,
        NewSemanticMutation {
            operation_id,
            kind,
            target_sequence: None,
            request_json: Some(request),
            transition,
            revision,
        },
    )?;
    transaction.commit()?;
    Ok(CommitOutcome {
        sequence,
        snapshot: graph,
    })
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

#[derive(Clone, Copy)]
struct NewSemanticMutation<'a> {
    operation_id: &'a str,
    kind: MutationKind,
    target_sequence: Option<i64>,
    request_json: Option<&'a str>,
    transition: &'a SemanticTransition,
    revision: u64,
}

fn append_semantic_mutation(
    connection: &Connection,
    mutation: NewSemanticMutation<'_>,
) -> Result<i64, StoreError> {
    connection.execute(
        "INSERT INTO mutation_log (operation_id, kind, target_sequence, revision, \
         request_json, prior_json, next_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            mutation.operation_id,
            mutation.kind.as_str(),
            mutation.target_sequence,
            to_sql_counter(mutation.revision)?,
            mutation.request_json,
            serde_json::to_string(&mutation.transition.prior)?,
            serde_json::to_string(&mutation.transition.next)?,
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

fn persist_semantic_transition(
    connection: &Connection,
    graph: &GraphSnapshot,
    touched: &SemanticState,
) -> Result<(), StoreError> {
    for (id, _) in &touched.bridges {
        match graph.bridges.get(id) {
            Some(bridge) => {
                let payload = serde_json::to_string(&serde_json::json!({
                    "from": bridge.from,
                    "to": bridge.to,
                    "label": bridge.label,
                    "provenance": bridge.provenance,
                }))?;
                connection.execute(
                    "INSERT INTO bridges (id, payload) VALUES (?1, ?2) \
                     ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
                    params![id.0, payload],
                )?;
            }
            None => {
                connection.execute("DELETE FROM bridges WHERE id=?1", [&id.0])?;
            }
        }
    }
    for (alias, _) in &touched.aliases {
        match graph.aliases.get(alias) {
            Some(canonical) => {
                connection.execute(
                    "INSERT INTO event_aliases (alias, canonical_id) VALUES (?1, ?2) \
                     ON CONFLICT(alias) DO UPDATE SET canonical_id=excluded.canonical_id",
                    params![alias.0, canonical.0],
                )?;
            }
            None => {
                connection.execute("DELETE FROM event_aliases WHERE alias=?1", [&alias.0])?;
            }
        }
    }
    for (id, _) in &touched.events {
        if let Some(event) = graph.events.get(id) {
            connection.execute(
                "INSERT INTO events (id, payload) VALUES (?1, ?2) \
                 ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
                params![id.0, serde_json::to_string(event)?],
            )?;
        } else {
            connection.execute("DELETE FROM placements WHERE event_id=?1", [&id.0])?;
            connection.execute("DELETE FROM placement_versions WHERE event_id=?1", [&id.0])?;
            connection.execute("DELETE FROM events WHERE id=?1", [&id.0])?;
        }
    }
    connection.execute(
        "UPDATE graph_meta SET value=?1 WHERE key='revision'",
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
        let kind = MutationKind::parse(&kind)?;
        if kind.is_action() {
            history.undo.push(sequence);
            history.redo.clear();
        } else {
            match kind {
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
                _ => unreachable!("action kinds were handled above"),
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

    fn curation_store() -> (TempDir, DurableGraphStore) {
        let (directory, path) = legacy_graph();
        Connection::open(&path)
            .unwrap()
            .execute(
                r#"UPDATE events SET payload =
                '{"id":"a","title":"A","date":"2026-07-14","color":4279312947,
                  "summary":"S","sourceLabel":"Source",
                  "artifacts":[{"text":"Primary paper","source":"Journal",
                    "url":"https://Example.test/Paper/#section"}]}'
                WHERE id='a'"#,
                [],
            )
            .unwrap();
        let store = DurableGraphStore::open(&path).unwrap();
        (directory, store)
    }

    fn relation(expected_revision: u64) -> RelateEvents {
        RelateEvents {
            bridge_id: BridgeId("user:a:b:supports".into()),
            from: EventId("a".into()),
            to: EventId("b".into()),
            predicate: "  Supports — with   evidence ".into(),
            provenance: Provenance::User,
            expected_revision,
        }
    }

    fn promotion(expected_revision: u64) -> PromoteArtifact {
        PromoteArtifact {
            source_event: EventId("a".into()),
            artifact_url: "https://example.test/paper".into(),
            promoted_id: EventId("paper".into()),
            date: "Jul 14, 2026".into(),
            relation: Some(not_news_domain::PromotionRelation {
                bridge_id: BridgeId("user:a:paper:cites".into()),
                predicate: "Cites as primary evidence".into(),
            }),
            expected_revision,
        }
    }

    #[test]
    fn mixed_command_history_survives_restart_and_restores_only_named_facts() {
        let (directory, store) = curation_store();
        let before = store.load().unwrap();
        let related = store
            .commit_relation("relate", &relation(before.revision))
            .unwrap();
        assert_eq!(
            related.snapshot.bridges[&BridgeId("user:a:b:supports".into())].label,
            "Supports - with evidence"
        );
        assert_eq!(related.snapshot.events, before.events);
        assert_eq!(
            related.snapshot.bridges[&BridgeId("a::b::related".into())],
            before.bridges[&BridgeId("a::b::related".into())]
        );

        drop(store);
        let reopened = DurableGraphStore::open(directory.path().join("graph.sqlite")).unwrap();
        let retry = reopened
            .commit_relation("relate", &relation(before.revision))
            .unwrap();
        assert_eq!(retry.sequence, related.sequence);
        let mut contradictory = relation(before.revision);
        contradictory.predicate = "Contradicts".into();
        assert!(matches!(
            reopened.commit_relation("relate", &contradictory),
            Err(StoreError::IdempotencyConflict(_))
        ));

        let detached = reopened
            .commit_detachment(
                "detach",
                &DetachRelationship {
                    bridge_id: BridgeId("user:a:b:supports".into()),
                    expected_revision: retry.snapshot.revision,
                },
            )
            .unwrap();
        assert!(
            !detached
                .snapshot
                .bridges
                .contains_key(&BridgeId("user:a:b:supports".into()))
        );
        let moved = reopened.commit_move("move", &command(75.0, 0)).unwrap();
        assert_eq!(moved.snapshot.revision, before.revision + 3);

        let without_move = reopened.undo("undo-move").unwrap().unwrap().snapshot;
        assert!(!without_move.placements.contains_key(&EventId("a".into())));
        let with_relation = reopened.undo("undo-detach").unwrap().unwrap().snapshot;
        assert!(
            with_relation
                .bridges
                .contains_key(&BridgeId("user:a:b:supports".into()))
        );
        let restored = reopened.undo("undo-relate").unwrap().unwrap().snapshot;
        assert_eq!(restored.events, before.events);
        assert_eq!(restored.bridges, before.bridges);
        assert_eq!(restored.aliases, before.aliases);
        assert_eq!(restored.placements, before.placements);

        let redone_relation = reopened.redo("redo-relate").unwrap().unwrap().snapshot;
        assert!(
            redone_relation
                .bridges
                .contains_key(&BridgeId("user:a:b:supports".into()))
        );
        let redone_detach = reopened.redo("redo-detach").unwrap().unwrap().snapshot;
        assert!(
            !redone_detach
                .bridges
                .contains_key(&BridgeId("user:a:b:supports".into()))
        );
        let redone_move = reopened.redo("redo-move").unwrap().unwrap().snapshot;
        assert!((redone_move.placements[&EventId("a".into())].point.x - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn promotion_deduplicates_primary_urls_and_refuses_dangling_inverse() {
        let (_directory, store) = curation_store();
        let before = store.load().unwrap();
        let promoted = store
            .commit_artifact_promotion("promote", &promotion(before.revision))
            .unwrap();
        assert_eq!(promoted.snapshot.events.len(), before.events.len() + 1);
        assert_eq!(
            promoted.snapshot.events[&EventId("paper".into())]
                .url
                .as_deref(),
            Some("https://Example.test/Paper/#section")
        );
        assert!(
            promoted
                .snapshot
                .bridges
                .contains_key(&BridgeId("user:a:paper:cites".into()))
        );

        let mut alias_promotion = promotion(promoted.snapshot.revision);
        alias_promotion.promoted_id = EventId("paper-alias".into());
        alias_promotion.relation = None;
        let aliased = store
            .commit_artifact_promotion("alias", &alias_promotion)
            .unwrap();
        assert_eq!(
            aliased.snapshot.events.len(),
            promoted.snapshot.events.len()
        );
        assert_eq!(
            aliased.snapshot.aliases[&EventId("paper-alias".into())],
            EventId("paper".into())
        );
        let without_alias = store.undo("undo-alias").unwrap().unwrap().snapshot;
        assert!(
            !without_alias
                .aliases
                .contains_key(&EventId("paper-alias".into()))
        );

        store
            .start_research_session("later", "Find corroboration")
            .unwrap();
        let dependent = store
            .accept_research_bridge(
                "later",
                0,
                &EventId("b".into()),
                &EventId("paper".into()),
                "Corroborates",
            )
            .unwrap()
            .snapshot;
        assert!(store.undo("unsafe-promotion-undo").is_err());
        assert_eq!(store.load().unwrap(), dependent);
        assert!(
            store
                .load()
                .unwrap()
                .events
                .contains_key(&EventId("paper".into()))
        );
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
    fn first_launch_creates_an_empty_graph_without_a_fake_migration_backup() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("graph.sqlite");
        let store = DurableGraphStore::open(&path).unwrap();
        assert!(store.migration_backup().is_none());
        assert_eq!(store.load().unwrap(), GraphSnapshot::default());
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert!(table_exists(&connection, "mutation_log").unwrap());
    }

    #[test]
    fn clear_erases_knowledge_history_and_research_in_one_retry_safe_revision() {
        let (_directory, path) = legacy_graph();
        let store = DurableGraphStore::open(&path).unwrap();
        store
            .commit_move("move-before-clear", &command(42.0, 0))
            .unwrap();
        store
            .start_research_session("research-before-clear", "Find evidence")
            .unwrap();
        store
            .record_research_output(
                "research-before-clear",
                0,
                crate::ResearchOutputKind::Message,
                "Searching",
            )
            .unwrap();
        let before = store.load().unwrap();

        let cleared = store.clear("clear-once").unwrap();
        assert!(cleared.events.is_empty());
        assert!(cleared.bridges.is_empty());
        assert!(cleared.aliases.is_empty());
        assert!(cleared.placements.is_empty());
        assert!(cleared.placement_versions.is_empty());
        assert_eq!(cleared.revision, before.revision + 1);
        assert_eq!(store.clear("clear-once").unwrap(), cleared);

        let connection = Connection::open(&path).unwrap();
        for table in [
            "events",
            "bridges",
            "event_aliases",
            "placements",
            "placement_versions",
            "mutation_log",
            "research_sessions",
            "research_output_log",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} survived destructive clear");
        }
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM destructive_log", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn a_late_clear_failure_restores_earlier_deletes_and_revision() {
        let (_directory, path) = legacy_graph();
        let store = DurableGraphStore::open(&path).unwrap();
        store
            .start_research_session("preserved-research", "Preserve this")
            .unwrap();
        store
            .record_research_output(
                "preserved-research",
                0,
                crate::ResearchOutputKind::Message,
                "Accepted activity",
            )
            .unwrap();
        let before = store.load().unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_event_clear BEFORE DELETE ON events \
                 BEGIN SELECT RAISE(ABORT, 'injected late clear failure'); END;",
            )
            .unwrap();

        assert!(store.clear("clear-must-rollback").is_err());
        assert_eq!(store.load().unwrap(), before);
        assert_eq!(
            store.research_activity("preserved-research").unwrap(),
            ["Accepted activity"]
        );
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM destructive_log", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn legacy_import_is_read_only_at_source_atomic_and_pristine_only() {
        let (_source_directory, source) = legacy_graph();
        Connection::open(&source)
            .unwrap()
            .execute("INSERT INTO placements VALUES ('a', 7.25, -8.5, 1)", [])
            .unwrap();
        let source_bytes = fs::read(&source).unwrap();
        let mut expected = crate::LegacyGraphReader::new(&source).load().unwrap();
        for event in expected.placements.keys() {
            expected
                .placement_versions
                .entry(event.clone())
                .or_insert(0);
        }
        let destination_directory = TempDir::new().unwrap();
        let destination = destination_directory.path().join("graph.sqlite");
        let store = DurableGraphStore::open(&destination).unwrap();

        assert_eq!(store.import_legacy(&source).unwrap(), expected);
        assert_eq!(store.load().unwrap(), expected);
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        assert!(matches!(
            store.import_legacy(&source),
            Err(StoreError::ImportDestinationNotEmpty)
        ));
    }

    #[test]
    fn failed_legacy_import_leaves_no_partial_destination() {
        let (_source_directory, source) = legacy_graph();
        let destination_directory = TempDir::new().unwrap();
        let destination = destination_directory.path().join("graph.sqlite");
        let store = DurableGraphStore::open(&destination).unwrap();
        Connection::open(&destination)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_import BEFORE INSERT ON events \
                 WHEN NEW.id='b' BEGIN SELECT RAISE(ABORT, 'injected import failure'); END;",
            )
            .unwrap();

        assert!(store.import_legacy(&source).is_err());
        assert_eq!(store.load().unwrap(), GraphSnapshot::default());
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
        for source in [
            std::env::var_os("NOT_NEWS_LEGACY_PRE_DRAG_DB")
                .map(PathBuf::from)
                .expect("NOT_NEWS_LEGACY_PRE_DRAG_DB must name the preserved pre-drag database"),
            std::env::var_os("NOT_NEWS_LEGACY_DRAG_DB")
                .map(PathBuf::from)
                .expect("NOT_NEWS_LEGACY_DRAG_DB must name the preserved drag-era database"),
        ] {
            let directory = TempDir::new().unwrap();
            let copy = directory.path().join("graph.sqlite");
            fs::copy(&source, &copy).unwrap();
            let before = crate::LegacyGraphReader::new(&copy).load().unwrap();
            let imported_path = directory.path().join("imported.sqlite");
            let imported_store = DurableGraphStore::open(&imported_path).unwrap();
            let imported = imported_store.import_legacy(&copy).unwrap();
            assert_knowledge_and_placement_equal(&before, &imported);
            drop(imported_store);
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
