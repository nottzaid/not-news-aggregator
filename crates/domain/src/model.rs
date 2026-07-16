use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub String);

impl EventId {
    /// Creates a nonempty event identifier.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::EmptyField`] when `value` is blank.
    pub fn new(value: impl Into<String>) -> Result<Self, SnapshotError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SnapshotError::EmptyField("event.id"));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BridgeId(pub String);

impl BridgeId {
    /// Creates a nonempty bridge identifier.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::EmptyField`] when `value` is blank.
    pub fn new(value: impl Into<String>) -> Result<Self, SnapshotError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SnapshotError::EmptyField("bridge.id"));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    /// Returns this point when both coordinates are finite.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotError::NonFinitePoint`] otherwise.
    pub fn validate(self) -> Result<Self, SnapshotError> {
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err(SnapshotError::NonFinitePoint);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub point: Point,
    pub pinned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceArtifact {
    pub text: String,
    pub source: String,
    pub url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResearchEvent {
    pub id: EventId,
    pub title: String,
    pub date: String,
    pub color: u32,
    pub summary: String,
    pub source_label: String,
    pub artifacts: Vec<SourceArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    #[default]
    Legacy,
    Agent,
    User,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventBridge {
    pub id: BridgeId,
    pub from: EventId,
    pub to: EventId,
    pub label: String,
    #[serde(default)]
    pub provenance: Provenance,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSnapshot {
    pub events: IndexMap<EventId, ResearchEvent>,
    pub bridges: IndexMap<BridgeId, EventBridge>,
    pub placements: IndexMap<EventId, Placement>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub placement_versions: IndexMap<EventId, u64>,
    pub aliases: IndexMap<EventId, EventId>,
    pub revision: u64,
}

impl GraphSnapshot {
    /// Verifies referential, identity, and field invariants.
    ///
    /// # Errors
    ///
    /// Returns the first violated [`SnapshotError`] invariant.
    pub fn validate(&self) -> Result<(), SnapshotError> {
        for (key, event) in &self.events {
            validate_nonempty(&key.0, "event key")?;
            if key != &event.id {
                return Err(SnapshotError::EventKeyMismatch {
                    key: key.clone(),
                    payload: event.id.clone(),
                });
            }
            validate_nonempty(&event.title, "event.title")?;
            validate_nonempty(&event.date, "event.date")?;
            validate_nonempty(&event.summary, "event.summary")?;
            validate_nonempty(&event.source_label, "event.sourceLabel")?;
            for artifact in &event.artifacts {
                validate_nonempty(&artifact.text, "artifact.text")?;
                validate_nonempty(&artifact.source, "artifact.source")?;
                validate_nonempty(&artifact.url, "artifact.url")?;
            }
        }

        for (key, bridge) in &self.bridges {
            validate_nonempty(&key.0, "bridge key")?;
            if key != &bridge.id {
                return Err(SnapshotError::BridgeKeyMismatch {
                    key: key.clone(),
                    payload: bridge.id.clone(),
                });
            }
            if bridge.from == bridge.to {
                return Err(SnapshotError::SelfLoop(bridge.id.clone()));
            }
            if !self.events.contains_key(&bridge.from) {
                return Err(SnapshotError::MissingEndpoint {
                    bridge: bridge.id.clone(),
                    event: bridge.from.clone(),
                });
            }
            if !self.events.contains_key(&bridge.to) {
                return Err(SnapshotError::MissingEndpoint {
                    bridge: bridge.id.clone(),
                    event: bridge.to.clone(),
                });
            }
            validate_nonempty(&bridge.label, "bridge.label")?;
        }

        for (event, placement) in &self.placements {
            if !self.events.contains_key(event) {
                return Err(SnapshotError::MissingPlacementEvent(event.clone()));
            }
            placement.point.validate()?;
        }

        for event in self.placement_versions.keys() {
            if !self.events.contains_key(event) {
                return Err(SnapshotError::MissingPlacementVersionEvent(event.clone()));
            }
        }

        for (alias, canonical) in &self.aliases {
            validate_nonempty(&alias.0, "event alias")?;
            if !self.events.contains_key(canonical) {
                return Err(SnapshotError::MissingAliasTarget {
                    alias: alias.clone(),
                    canonical: canonical.clone(),
                });
            }
        }
        Ok(())
    }
}

fn validate_nonempty(value: &str, field: &'static str) -> Result<(), SnapshotError> {
    if value.trim().is_empty() {
        Err(SnapshotError::EmptyField(field))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SnapshotError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("point coordinates must be finite")]
    NonFinitePoint,
    #[error("event key {key:?} differs from payload ID {payload:?}")]
    EventKeyMismatch { key: EventId, payload: EventId },
    #[error("bridge key {key:?} differs from payload ID {payload:?}")]
    BridgeKeyMismatch { key: BridgeId, payload: BridgeId },
    #[error("bridge {0:?} is a self-loop")]
    SelfLoop(BridgeId),
    #[error("bridge {bridge:?} references missing event {event:?}")]
    MissingEndpoint { bridge: BridgeId, event: EventId },
    #[error("placement references missing event {0:?}")]
    MissingPlacementEvent(EventId),
    #[error("placement version references missing event {0:?}")]
    MissingPlacementVersionEvent(EventId),
    #[error("alias {alias:?} references missing canonical event {canonical:?}")]
    MissingAliasTarget { alias: EventId, canonical: EventId },
}
