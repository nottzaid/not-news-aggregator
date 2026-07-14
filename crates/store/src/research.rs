use not_news_domain::{BridgeId, EventBridge, EventId, Provenance, ResearchEvent};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

use crate::durable::open_connection;
use crate::{DurableGraphStore, StoreError, load_snapshot};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchSessionStatus {
    Running,
    Done,
    Error,
    Interrupted,
}

impl ResearchSessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Error => "error",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "running" => Ok(Self::Running),
            "done" => Ok(Self::Done),
            "error" => Ok(Self::Error),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(StoreError::InvalidField("research_sessions.status")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResearchSession {
    pub id: String,
    pub prompt: String,
    pub status: ResearchSessionStatus,
    pub last_output_sequence: Option<u64>,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResearchOutputKind {
    Message,
    VoiceNote,
    ProtocolError,
}

impl ResearchOutputKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::VoiceNote => "voice_note",
            Self::ProtocolError => "protocol_error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcceptedResearchMutation {
    pub log_sequence: i64,
    pub canonical_key: String,
    pub changed: bool,
    pub snapshot: not_news_domain::GraphSnapshot,
}

impl DurableGraphStore {
    /// Starts a durable research session. An exact retry is harmless; reusing
    /// the ID for another prompt fails.
    ///
    /// # Errors
    ///
    /// Rejects blank input, conflicting ID reuse, unavailable storage, or an
    /// invalid migrated database.
    pub fn start_research_session(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<ResearchSession, StoreError> {
        validate_session_id(session_id)?;
        if prompt.trim().is_empty() {
            return Err(StoreError::EmptyResearchPrompt);
        }
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = session_by_id(&transaction, session_id)? {
            if existing.prompt != prompt {
                return Err(StoreError::ResearchOutputConflict {
                    session: session_id.to_owned(),
                    sequence: 0,
                });
            }
            transaction.commit()?;
            return Ok(existing);
        }
        transaction.execute(
            "INSERT INTO research_sessions (id, prompt, status) VALUES (?1, ?2, 'running')",
            params![session_id, prompt],
        )?;
        let session = session_by_id(&transaction, session_id)?
            .ok_or_else(|| StoreError::MissingResearchSession(session_id.to_owned()))?;
        transaction.commit()?;
        Ok(session)
    }

    /// Marks sessions whose owning process disappeared as interrupted. Accepted
    /// graph mutations remain intact and the prompt remains available to retry.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be read or updated atomically.
    pub fn recover_interrupted_research(&self) -> Result<Vec<ResearchSession>, StoreError> {
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT id FROM research_sessions WHERE status='running' ORDER BY rowid",
            )?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        transaction.execute(
            "UPDATE research_sessions SET status='interrupted', \
             message='Research stopped when the application exited.', updated_at=unixepoch() \
             WHERE status='running'",
            [],
        )?;
        let sessions = ids
            .iter()
            .map(|id| {
                session_by_id(&transaction, id)?
                    .ok_or_else(|| StoreError::MissingResearchSession(id.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit()?;
        Ok(sessions)
    }

    /// Reads the bounded human-facing activity trail for one durable session.
    /// Graph proposal payloads remain in the audit log but are not duplicated as
    /// progress prose.
    ///
    /// # Errors
    ///
    /// Returns an error for an unavailable database, missing session, malformed
    /// stored message JSON, or invalid output state.
    pub fn research_activity(&self, session_id: &str) -> Result<Vec<String>, StoreError> {
        validate_session_id(session_id)?;
        let connection = open_connection(&self.path)?;
        require_session(&connection, session_id)?;
        let mut statement = connection.prepare(
            "SELECT payload FROM research_output_log \
             WHERE session_id=?1 AND kind IN ('message', 'voice_note', 'protocol_error') \
             ORDER BY output_sequence DESC LIMIT 80",
        )?;
        let rows = statement.query_map([session_id], |row| row.get::<_, String>(0))?;
        let mut messages = rows
            .map(|row| Ok(serde_json::from_str::<String>(&row?)?))
            .collect::<Result<Vec<_>, StoreError>>()?;
        messages.reverse();
        Ok(messages)
    }

    /// Records non-mutating typed output in the same ordered stream used for
    /// accepted graph proposals.
    ///
    /// # Errors
    ///
    /// Rejects blank, missing, closed, out-of-order, or conflicting output and
    /// failed durable transactions.
    pub fn record_research_output(
        &self,
        session_id: &str,
        output_sequence: u64,
        kind: ResearchOutputKind,
        message: &str,
    ) -> Result<ResearchSession, StoreError> {
        if message.trim().is_empty() {
            return Err(StoreError::EmptyResearchMessage);
        }
        let payload = serde_json::to_string(message)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match prepare_output(
            &transaction,
            session_id,
            output_sequence,
            kind.as_str(),
            &payload,
        )? {
            PreparedOutput::Existing(_) => {
                let session = require_session(&transaction, session_id)?;
                transaction.commit()?;
                return Ok(session);
            }
            PreparedOutput::New(session) => {
                let revision = load_snapshot(&transaction)?.revision;
                append_output(
                    &transaction,
                    session_id,
                    output_sequence,
                    kind.as_str(),
                    &payload,
                    None,
                    revision,
                )?;
                advance_session(&transaction, &session, output_sequence, None, Some(message))?;
            }
        }
        let session = require_session(&transaction, session_id)?;
        transaction.commit()?;
        Ok(session)
    }

    /// Accepts one validated event proposal and its audit row atomically.
    ///
    /// # Errors
    ///
    /// Rejects missing, closed, out-of-order, conflicting, or invalid output and
    /// rolls back any failed graph/log transaction.
    pub fn accept_research_event(
        &self,
        session_id: &str,
        output_sequence: u64,
        event: &ResearchEvent,
    ) -> Result<AcceptedResearchMutation, StoreError> {
        let payload = serde_json::to_string(event)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let PreparedOutput::Existing(existing) =
            prepare_output(&transaction, session_id, output_sequence, "event", &payload)?
        {
            let snapshot = load_snapshot(&transaction)?;
            transaction.commit()?;
            return Ok(AcceptedResearchMutation {
                log_sequence: existing.log_sequence,
                canonical_key: existing.canonical_key.unwrap_or_default(),
                changed: false,
                snapshot,
            });
        }

        let (canonical, changed) = upsert_event(&transaction, event)?;
        let revision = bump_revision_if(&transaction, changed)?;
        let log_sequence = append_output(
            &transaction,
            session_id,
            output_sequence,
            "event",
            &payload,
            Some(&canonical.0),
            revision,
        )?;
        let session = require_session(&transaction, session_id)?;
        advance_session(&transaction, &session, output_sequence, None, None)?;
        let snapshot = load_snapshot(&transaction)?;
        transaction.commit()?;
        Ok(AcceptedResearchMutation {
            log_sequence,
            canonical_key: canonical.0,
            changed,
            snapshot,
        })
    }

    /// Accepts one validated bridge proposal only after both aliases resolve to
    /// distinct durable events; rejection cannot leave a log or partial edge.
    ///
    /// # Errors
    ///
    /// Rejects missing endpoints, self-loops, blank labels, invalid sequencing,
    /// conflicting retries, closed sessions, and failed durable transactions.
    pub fn accept_research_bridge(
        &self,
        session_id: &str,
        output_sequence: u64,
        from: &EventId,
        to: &EventId,
        label: &str,
    ) -> Result<AcceptedResearchMutation, StoreError> {
        let normalized_label = normalize_bridge_label(label);
        if normalized_label.is_empty() {
            return Err(StoreError::InvalidField("bridge.label"));
        }
        let payload = serde_json::to_string(&(from, to, &normalized_label))?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let PreparedOutput::Existing(existing) = prepare_output(
            &transaction,
            session_id,
            output_sequence,
            "bridge",
            &payload,
        )? {
            let snapshot = load_snapshot(&transaction)?;
            transaction.commit()?;
            return Ok(AcceptedResearchMutation {
                log_sequence: existing.log_sequence,
                canonical_key: existing.canonical_key.unwrap_or_default(),
                changed: false,
                snapshot,
            });
        }

        let resolved_from = resolve_event_id(&transaction, from)?;
        let resolved_to = resolve_event_id(&transaction, to)?;
        if resolved_from == resolved_to {
            return Err(StoreError::ResearchSelfLoop(resolved_from));
        }
        let key = BridgeId(format!(
            "{}::{}::{}",
            resolved_from.0,
            resolved_to.0,
            normalized_label.to_lowercase()
        ));
        let bridge = EventBridge {
            id: key.clone(),
            from: resolved_from,
            to: resolved_to,
            label: normalized_label,
            provenance: Provenance::Agent,
        };
        let serialized = serde_json::to_string(&serde_json::json!({
            "from": bridge.from,
            "to": bridge.to,
            "label": bridge.label,
            "provenance": bridge.provenance,
        }))?;
        let previous: Option<String> = transaction
            .query_row("SELECT payload FROM bridges WHERE id=?1", [&key.0], |row| {
                row.get(0)
            })
            .optional()?;
        let changed = previous.as_deref() != Some(serialized.as_str());
        if changed {
            transaction.execute(
                "INSERT INTO bridges (id, payload) VALUES (?1, ?2) \
                 ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
                params![key.0, serialized],
            )?;
        }
        let revision = bump_revision_if(&transaction, changed)?;
        let log_sequence = append_output(
            &transaction,
            session_id,
            output_sequence,
            "bridge",
            &payload,
            Some(&key.0),
            revision,
        )?;
        let session = require_session(&transaction, session_id)?;
        advance_session(&transaction, &session, output_sequence, None, None)?;
        let snapshot = load_snapshot(&transaction)?;
        transaction.commit()?;
        Ok(AcceptedResearchMutation {
            log_sequence,
            canonical_key: key.0,
            changed,
            snapshot,
        })
    }

    /// Completes a running session and logs the terminal output atomically.
    ///
    /// # Errors
    ///
    /// Rejects non-terminal status, blank messages, invalid sequencing,
    /// conflicting retries, closed sessions, and failed durable transactions.
    pub fn finish_research_session(
        &self,
        session_id: &str,
        output_sequence: u64,
        status: ResearchSessionStatus,
        message: &str,
    ) -> Result<ResearchSession, StoreError> {
        if !matches!(
            status,
            ResearchSessionStatus::Done | ResearchSessionStatus::Error
        ) {
            return Err(StoreError::InvalidField("research_sessions.status"));
        }
        if message.trim().is_empty() {
            return Err(StoreError::EmptyResearchMessage);
        }
        let kind = status.as_str();
        let payload = serde_json::to_string(message)?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let PreparedOutput::Existing(_) =
            prepare_output(&transaction, session_id, output_sequence, kind, &payload)?
        {
            let session = require_session(&transaction, session_id)?;
            transaction.commit()?;
            return Ok(session);
        }
        let revision = load_snapshot(&transaction)?.revision;
        append_output(
            &transaction,
            session_id,
            output_sequence,
            kind,
            &payload,
            None,
            revision,
        )?;
        let session = require_session(&transaction, session_id)?;
        advance_session(
            &transaction,
            &session,
            output_sequence,
            Some(status),
            Some(message),
        )?;
        let session = require_session(&transaction, session_id)?;
        transaction.commit()?;
        Ok(session)
    }
}

#[derive(Debug)]
enum PreparedOutput {
    Existing(ExistingOutput),
    New(ResearchSession),
}

#[derive(Debug)]
struct ExistingOutput {
    log_sequence: i64,
    canonical_key: Option<String>,
}

fn prepare_output(
    connection: &Connection,
    session_id: &str,
    output_sequence: u64,
    kind: &str,
    payload: &str,
) -> Result<PreparedOutput, StoreError> {
    validate_session_id(session_id)?;
    let output_sequence_sql = to_sql_counter(output_sequence)?;
    let existing = connection
        .query_row(
            "SELECT sequence, kind, payload, canonical_key FROM research_output_log \
             WHERE session_id=?1 AND output_sequence=?2",
            params![session_id, output_sequence_sql],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((log_sequence, existing_kind, existing_payload, canonical_key)) = existing {
        if existing_kind != kind || existing_payload != payload {
            return Err(StoreError::ResearchOutputConflict {
                session: session_id.to_owned(),
                sequence: output_sequence,
            });
        }
        return Ok(PreparedOutput::Existing(ExistingOutput {
            log_sequence,
            canonical_key,
        }));
    }
    let session = require_session(connection, session_id)?;
    if session.status != ResearchSessionStatus::Running {
        return Err(StoreError::ResearchSessionClosed {
            session: session_id.to_owned(),
            status: session.status.as_str().to_owned(),
        });
    }
    let expected = session
        .last_output_sequence
        .map_or(0, |sequence| sequence + 1);
    if output_sequence != expected {
        return Err(StoreError::ResearchSequenceConflict {
            session: session_id.to_owned(),
            expected,
            actual: output_sequence,
        });
    }
    Ok(PreparedOutput::New(session))
}

fn append_output(
    connection: &Connection,
    session_id: &str,
    output_sequence: u64,
    kind: &str,
    payload: &str,
    canonical_key: Option<&str>,
    revision: u64,
) -> Result<i64, StoreError> {
    connection.execute(
        "INSERT INTO research_output_log \
         (session_id, output_sequence, kind, payload, canonical_key, graph_revision) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            session_id,
            to_sql_counter(output_sequence)?,
            kind,
            payload,
            canonical_key,
            to_sql_counter(revision)?,
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

fn advance_session(
    connection: &Connection,
    session: &ResearchSession,
    output_sequence: u64,
    status: Option<ResearchSessionStatus>,
    message: Option<&str>,
) -> Result<(), StoreError> {
    connection.execute(
        "UPDATE research_sessions SET last_output_sequence=?1, status=?2, \
         message=COALESCE(?3, message), updated_at=unixepoch() WHERE id=?4",
        params![
            to_sql_counter(output_sequence)?,
            status.unwrap_or(session.status).as_str(),
            message,
            session.id,
        ],
    )?;
    Ok(())
}

fn upsert_event(
    connection: &Connection,
    incoming: &ResearchEvent,
) -> Result<(EventId, bool), StoreError> {
    let canonical = canonical_event_id(connection, incoming)?;
    if canonical != incoming.id {
        let previous: Option<String> = connection
            .query_row(
                "SELECT canonical_id FROM event_aliases WHERE alias=?1",
                [&incoming.id.0],
                |row| row.get(0),
            )
            .optional()?;
        connection.execute(
            "INSERT INTO event_aliases (alias, canonical_id) VALUES (?1, ?2) \
             ON CONFLICT(alias) DO UPDATE SET canonical_id=excluded.canonical_id",
            params![incoming.id.0, canonical.0],
        )?;
        let changed = previous.as_deref() != Some(canonical.0.as_str());
        return Ok((canonical, changed));
    }

    let mut event = incoming.clone();
    let mut used = stored_urls_except(connection, &canonical)?;
    if let Some(primary) = event
        .url
        .as_deref()
        .map(normalize_url)
        .filter(|url| !url.is_empty())
    {
        used.insert(primary);
    }
    event.artifacts.retain(|artifact| {
        let url = normalize_url(&artifact.url);
        !url.is_empty() && used.insert(url)
    });
    let serialized = serde_json::to_string(&event)?;
    let previous: Option<String> = connection
        .query_row(
            "SELECT payload FROM events WHERE id=?1",
            [&canonical.0],
            |row| row.get(0),
        )
        .optional()?;
    let changed = previous.as_deref() != Some(serialized.as_str());
    if changed {
        connection.execute(
            "INSERT INTO events (id, payload) VALUES (?1, ?2) \
             ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
            params![canonical.0, serialized],
        )?;
    }
    Ok((canonical, changed))
}

fn canonical_event_id(
    connection: &Connection,
    incoming: &ResearchEvent,
) -> Result<EventId, StoreError> {
    let Some(primary) = incoming
        .url
        .as_deref()
        .map(normalize_url)
        .filter(|url| !url.is_empty())
    else {
        return Ok(incoming.id.clone());
    };
    let mut statement = connection.prepare("SELECT id, payload FROM events ORDER BY rowid")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, payload) = row?;
        if id == incoming.id.0 {
            return Ok(incoming.id.clone());
        }
        let value: Value = serde_json::from_str(&payload)?;
        if event_urls(&value).contains(&primary) {
            return Ok(EventId(id));
        }
    }
    Ok(incoming.id.clone())
}

fn stored_urls_except(
    connection: &Connection,
    excluded: &EventId,
) -> Result<std::collections::HashSet<String>, StoreError> {
    let mut statement = connection.prepare("SELECT payload FROM events WHERE id != ?1")?;
    let rows = statement.query_map([&excluded.0], |row| row.get::<_, String>(0))?;
    let mut urls = std::collections::HashSet::new();
    for row in rows {
        urls.extend(event_urls(&serde_json::from_str(&row?)?));
    }
    Ok(urls)
}

fn event_urls(event: &Value) -> std::collections::HashSet<String> {
    let mut urls = std::collections::HashSet::new();
    if let Some(url) = event.get("url").and_then(Value::as_str) {
        let normalized = normalize_url(url);
        if !normalized.is_empty() {
            urls.insert(normalized);
        }
    }
    if let Some(artifacts) = event.get("artifacts").and_then(Value::as_array) {
        for artifact in artifacts {
            if let Some(url) = artifact.get("url").and_then(Value::as_str) {
                let normalized = normalize_url(url);
                if !normalized.is_empty() {
                    urls.insert(normalized);
                }
            }
        }
    }
    urls
}

pub(crate) fn normalize_url(url: &str) -> String {
    url.trim()
        .split('#')
        .next()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_lowercase()
}

fn resolve_event_id(connection: &Connection, id: &EventId) -> Result<EventId, StoreError> {
    if connection
        .query_row("SELECT 1 FROM events WHERE id=?1", [&id.0], |_| Ok(()))
        .optional()?
        .is_some()
    {
        return Ok(id.clone());
    }
    connection
        .query_row(
            "SELECT canonical_id FROM event_aliases WHERE alias=?1 \
             AND EXISTS (SELECT 1 FROM events WHERE id=canonical_id)",
            [&id.0],
            |row| row.get::<_, String>(0).map(EventId),
        )
        .optional()?
        .ok_or_else(|| StoreError::MissingResearchEndpoint(id.clone()))
}

fn normalize_bridge_label(label: &str) -> String {
    label
        .replace(['—', '–'], "-")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bump_revision_if(connection: &Connection, changed: bool) -> Result<u64, StoreError> {
    let mut graph = load_snapshot(connection)?;
    if changed {
        graph.revision = graph
            .revision
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        connection.execute(
            "UPDATE graph_meta SET value=?1 WHERE key='revision'",
            [graph.revision.to_string()],
        )?;
    }
    Ok(graph.revision)
}

fn require_session(
    connection: &Connection,
    session_id: &str,
) -> Result<ResearchSession, StoreError> {
    session_by_id(connection, session_id)?
        .ok_or_else(|| StoreError::MissingResearchSession(session_id.to_owned()))
}

fn session_by_id(
    connection: &Connection,
    session_id: &str,
) -> Result<Option<ResearchSession>, StoreError> {
    connection
        .query_row(
            "SELECT id, prompt, status, last_output_sequence, message \
             FROM research_sessions WHERE id=?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?
        .map(|(id, prompt, status, sequence, message)| {
            Ok(ResearchSession {
                id,
                prompt,
                status: ResearchSessionStatus::parse(&status)?,
                last_output_sequence: sequence
                    .map(|value| {
                        u64::try_from(value).map_err(|_| {
                            StoreError::InvalidField("research_sessions.last_output_sequence")
                        })
                    })
                    .transpose()?,
                message,
            })
        })
        .transpose()
}

fn validate_session_id(session_id: &str) -> Result<(), StoreError> {
    if session_id.trim().is_empty() {
        Err(StoreError::EmptySessionId)
    } else {
        Ok(())
    }
}

fn to_sql_counter(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::CounterTooLarge(value))
}

#[cfg(test)]
mod tests {
    use not_news_domain::SourceArtifact;
    use rusqlite::Connection;
    use tempfile::TempDir;

    use super::*;
    use crate::table_exists;

    fn store() -> (TempDir, DurableGraphStore) {
        let directory = TempDir::new().unwrap();
        let store = DurableGraphStore::open(directory.path().join("graph.sqlite")).unwrap();
        (directory, store)
    }

    fn event(id: &str, url: &str, artifacts: &[&str]) -> ResearchEvent {
        ResearchEvent {
            id: EventId(id.into()),
            title: format!("Finding {id}"),
            date: "Jul 14, 2026".into(),
            color: 0xff4c_c9d6,
            summary: "Evidence-backed summary.".into(),
            source_label: "Primary".into(),
            artifacts: artifacts
                .iter()
                .map(|url| SourceArtifact {
                    text: "Source".into(),
                    source: "Primary".into(),
                    url: (*url).into(),
                })
                .collect(),
            url: Some(url.into()),
        }
    }

    #[test]
    fn ordered_outputs_are_idempotent_and_closed_sessions_reject_new_work() {
        let (_directory, store) = store();
        let started = store
            .start_research_session("session", "Investigate")
            .unwrap();
        assert_eq!(started.status, ResearchSessionStatus::Running);

        let progress = store
            .record_research_output("session", 0, ResearchOutputKind::Message, "Searching")
            .unwrap();
        assert_eq!(progress.last_output_sequence, Some(0));
        let retry = store
            .record_research_output("session", 0, ResearchOutputKind::Message, "Searching")
            .unwrap();
        assert_eq!(retry, progress);
        assert!(matches!(
            store.record_research_output("session", 0, ResearchOutputKind::Message, "Different"),
            Err(StoreError::ResearchOutputConflict { .. })
        ));
        assert!(matches!(
            store.record_research_output("session", 2, ResearchOutputKind::Message, "Skipped"),
            Err(StoreError::ResearchSequenceConflict {
                expected: 1,
                actual: 2,
                ..
            })
        ));

        let done = store
            .finish_research_session("session", 1, ResearchSessionStatus::Done, "Complete")
            .unwrap();
        assert_eq!(done.status, ResearchSessionStatus::Done);
        assert!(matches!(
            store.record_research_output("session", 2, ResearchOutputKind::Message, "Too late"),
            Err(StoreError::ResearchSessionClosed { .. })
        ));
    }

    #[test]
    fn event_identity_and_global_source_dedupe_match_the_preserved_contract() {
        let (_directory, store) = store();
        store
            .start_research_session("session", "Investigate")
            .unwrap();
        let first = store
            .accept_research_event(
                "session",
                0,
                &event(
                    "a",
                    "HTTPS://Example.test/Finding/#fragment",
                    &[
                        "https://example.test/evidence",
                        "https://example.test/finding",
                    ],
                ),
            )
            .unwrap();
        assert!(first.changed);
        assert_eq!(first.snapshot.revision, 1);
        assert_eq!(
            first.snapshot.events[&EventId("a".into())].artifacts.len(),
            1
        );

        let alias = store
            .accept_research_event(
                "session",
                1,
                &event("duplicate", "https://example.test/finding", &[]),
            )
            .unwrap();
        assert_eq!(alias.canonical_key, "a");
        assert_eq!(
            alias.snapshot.aliases[&EventId("duplicate".into())],
            EventId("a".into())
        );
        assert_eq!(alias.snapshot.events.len(), 1);

        let third_proposal = event(
            "c",
            "https://example.test/other",
            &[
                "https://example.test/evidence#second",
                "https://example.test/unique",
            ],
        );
        let third = store
            .accept_research_event("session", 2, &third_proposal)
            .unwrap();
        assert_eq!(
            third.snapshot.events[&EventId("c".into())].artifacts.len(),
            1
        );
        assert_eq!(third.snapshot.revision, 3);
        let retry = store
            .accept_research_event("session", 2, &third_proposal)
            .unwrap();
        assert_eq!(retry.log_sequence, third.log_sequence);
        assert_eq!(retry.snapshot.revision, 3);
        assert!(matches!(
            store.accept_research_event(
                "session",
                2,
                &event("different", "https://different.test", &[])
            ),
            Err(StoreError::ResearchOutputConflict { .. })
        ));
    }

    #[test]
    fn bridge_acceptance_resolves_aliases_and_rejection_is_non_mutating() {
        let (_directory, store) = store();
        store
            .start_research_session("session", "Investigate")
            .unwrap();
        store
            .accept_research_event("session", 0, &event("a", "https://example.test/a", &[]))
            .unwrap();
        store
            .accept_research_event(
                "session",
                1,
                &event("alias-a", "https://example.test/a", &[]),
            )
            .unwrap();
        store
            .accept_research_event("session", 2, &event("b", "https://example.test/b", &[]))
            .unwrap();

        assert!(matches!(
            store.accept_research_bridge(
                "session",
                3,
                &EventId("alias-a".into()),
                &EventId("a".into()),
                "same"
            ),
            Err(StoreError::ResearchSelfLoop(_))
        ));
        assert!(matches!(
            store.accept_research_bridge(
                "session",
                3,
                &EventId("missing".into()),
                &EventId("b".into()),
                "missing"
            ),
            Err(StoreError::MissingResearchEndpoint(_))
        ));
        let accepted = store
            .accept_research_bridge(
                "session",
                3,
                &EventId("alias-a".into()),
                &EventId("b".into()),
                "  Supports — with   evidence  ",
            )
            .unwrap();
        assert_eq!(accepted.canonical_key, "a::b::supports - with evidence");
        let bridge = &accepted.snapshot.bridges[&BridgeId(accepted.canonical_key.clone())];
        assert_eq!(bridge.from, EventId("a".into()));
        assert_eq!(bridge.label, "Supports - with evidence");
        assert_eq!(bridge.provenance, Provenance::Agent);
        assert_eq!(accepted.snapshot.revision, 4);
    }

    #[test]
    fn output_log_failure_rolls_back_the_graph_and_session_cursor() {
        let (_directory, store) = store();
        store
            .start_research_session("session", "Investigate")
            .unwrap();
        let connection = Connection::open(store.path()).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_research_log BEFORE INSERT ON research_output_log \
                 BEGIN SELECT RAISE(ABORT, 'injected crash boundary'); END;",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            store.accept_research_event("session", 0, &event("a", "https://example.test/a", &[])),
            Err(StoreError::Sqlite(_))
        ));
        assert!(store.load().unwrap().events.is_empty());
        assert_eq!(store.load().unwrap().revision, 0);
        let connection = Connection::open(store.path()).unwrap();
        let cursor: Option<i64> = connection
            .query_row(
                "SELECT last_output_sequence FROM research_sessions WHERE id='session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursor, None);
    }

    #[test]
    fn reopening_marks_only_abandoned_running_sessions_interrupted() {
        let (_directory, store) = store();
        store.start_research_session("running", "One").unwrap();
        store
            .record_research_output(
                "running",
                0,
                ResearchOutputKind::Message,
                "Searching primary sources",
            )
            .unwrap();
        store.start_research_session("done", "Two").unwrap();
        store
            .finish_research_session("done", 0, ResearchSessionStatus::Done, "Complete")
            .unwrap();

        let recovered = store.recover_interrupted_research().unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, "running");
        assert_eq!(recovered[0].status, ResearchSessionStatus::Interrupted);
        assert_eq!(
            store.research_activity("running").unwrap(),
            ["Searching primary sources"]
        );
        assert!(store.recover_interrupted_research().unwrap().is_empty());
    }

    #[test]
    fn version_one_upgrade_has_a_verified_backup_and_preserves_history() {
        let (directory, store) = store();
        let path = store.path().to_owned();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE destructive_log; DROP TABLE research_output_log; \
                 DROP TABLE research_sessions; \
                 PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);
        drop(store);

        let upgraded = DurableGraphStore::open(&path).unwrap();
        let backup = upgraded.migration_backup().unwrap();
        let backup_connection = Connection::open(backup).unwrap();
        assert_eq!(
            backup_connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!table_exists(&backup_connection, "research_sessions").unwrap());
        assert!(!table_exists(&backup_connection, "destructive_log").unwrap());
        assert!(table_exists(&backup_connection, "mutation_log").unwrap());
        let current = Connection::open(directory.path().join("graph.sqlite")).unwrap();
        assert!(table_exists(&current, "research_sessions").unwrap());
        assert_eq!(
            current
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert!(table_exists(&current, "destructive_log").unwrap());
    }
}
