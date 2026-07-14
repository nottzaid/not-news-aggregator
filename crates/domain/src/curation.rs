use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BridgeId, EventId, Provenance};

pub const MAX_PREDICATE_CHARS: usize = 96;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelateEvents {
    pub bridge_id: BridgeId,
    pub from: EventId,
    pub to: EventId,
    pub predicate: String,
    pub provenance: Provenance,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachRelationship {
    pub bridge_id: BridgeId,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionRelation {
    pub bridge_id: BridgeId,
    pub predicate: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteArtifact {
    pub source_event: EventId,
    pub artifact_url: String,
    pub promoted_id: EventId,
    pub date: String,
    pub relation: Option<PromotionRelation>,
    pub expected_revision: u64,
}

/// Collapses typographic dashes and whitespace without changing word case.
///
/// # Errors
///
/// Rejects blank predicates, control characters, or more than 96 Unicode
/// scalar values after normalization.
pub fn normalize_predicate(value: &str) -> Result<String, CurationCommandError> {
    if value.chars().any(char::is_control) {
        return Err(CurationCommandError::ControlCharacter);
    }
    let normalized = value
        .replace(['—', '–'], "-")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return Err(CurationCommandError::EmptyPredicate);
    }
    if normalized.chars().count() > MAX_PREDICATE_CHARS {
        return Err(CurationCommandError::PredicateTooLong);
    }
    Ok(normalized)
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CurationCommandError {
    #[error("relationship predicate must not be blank")]
    EmptyPredicate,
    #[error("relationship predicate must not contain control characters")]
    ControlCharacter,
    #[error("relationship predicate exceeds {MAX_PREDICATE_CHARS} characters")]
    PredicateTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicate_normalization_is_bounded_without_flattening_case() {
        assert_eq!(
            normalize_predicate("  Supports — with   evidence ").unwrap(),
            "Supports - with evidence"
        );
        assert!(matches!(
            normalize_predicate("has\na newline"),
            Err(CurationCommandError::ControlCharacter)
        ));
        assert!(matches!(
            normalize_predicate(&"x".repeat(MAX_PREDICATE_CHARS + 1)),
            Err(CurationCommandError::PredicateTooLong)
        ));
    }
}
