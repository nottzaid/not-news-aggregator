//! Bounded external-agent integration.

mod compatibility;
mod prompt;
mod runner;

use not_news_domain::{EventId, GraphSnapshot, ResearchEvent, SnapshotError, SourceArtifact};
use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

pub use compatibility::{
    HermesCompatibility, HermesCompatibilityError, ToolCapabilityError, check_hermes_compatibility,
    check_tool_capability,
};
pub use prompt::build_research_prompt;
pub use runner::{
    HermesDashboardError, OutputProtocol, ProcessLimits, ResearchBackend, ResearchHandle,
    ResearchLaunch, ResearchProcessEvent, ResearchTermination, ResolvedEnvironment,
    browse_is_available, curl_is_available, hermes_is_available, open_hermes_dashboard,
};

pub const EVENT_PREFIX: &str = "AI_NEWS_EVENT:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeUpsert {
    pub from: EventId,
    pub to: EventId,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentEvent {
    EventUpsert(ResearchEvent),
    BridgeUpsert(BridgeUpsert),
    SessionMessage(String),
    SessionError(String),
    SessionDone(String),
    VoiceNote(String),
}

#[derive(Debug, Error)]
pub enum AgentProtocolError {
    #[error("Hermes event is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Hermes event type {0:?}")]
    UnsupportedType(String),
    #[error("Hermes {0} payload must be an object")]
    Object(&'static str),
    #[error("Hermes payload is missing nonempty field {0}")]
    Field(&'static str),
    #[error("Hermes color {0} must be an ARGB integer or six/eight-digit hex string")]
    Color(String),
    #[error("Hermes event violates graph invariants: {0}")]
    Snapshot(#[from] SnapshotError),
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    data: Value,
}

/// Parses one cleaned Hermes output line without mutating application state.
/// Ordinary prose becomes progress; prefixed JSON is normalized into typed
/// proposals that still require store acceptance.
///
/// # Errors
///
/// Rejects malformed JSON, unsupported types, invalid fields, and event graph
/// invariant violations.
pub fn parse_output_line(line: &str) -> Result<AgentEvent, AgentProtocolError> {
    let Some(raw) = line.strip_prefix(EVENT_PREFIX) else {
        return nonempty(line, "message").map(|message| AgentEvent::SessionMessage(message.into()));
    };
    let envelope: Envelope = serde_json::from_str(raw.trim())?;
    match envelope.event_type.as_str() {
        "event.upsert" => parse_event(envelope.data).map(AgentEvent::EventUpsert),
        "bridge.upsert" => parse_bridge(envelope.data).map(AgentEvent::BridgeUpsert),
        "session.message" => message(envelope.data).map(AgentEvent::SessionMessage),
        "session.error" => message(envelope.data).map(AgentEvent::SessionError),
        "session.done" => message(envelope.data).map(AgentEvent::SessionDone),
        "voice.note" => message(envelope.data).map(AgentEvent::VoiceNote),
        other => Err(AgentProtocolError::UnsupportedType(other.to_owned())),
    }
}

fn parse_event(data: Value) -> Result<ResearchEvent, AgentProtocolError> {
    let mut object = into_object(data, "event.upsert")?;
    let source = required_string(&object, "sourceLabel")?.to_owned();
    let color = parse_color(object.get("color"))?;
    object.insert("color".into(), Value::from(color));
    let artifacts = object
        .remove("artifacts")
        .unwrap_or_else(|| Value::Array(Vec::new()));
    object.insert(
        "artifacts".into(),
        Value::Array(normalize_artifacts(artifacts, &source)?),
    );
    let event: ResearchEvent = serde_json::from_value(Value::Object(object))?;
    if event
        .url
        .as_deref()
        .is_some_and(|url| url.trim().is_empty())
    {
        return Err(AgentProtocolError::Field("url"));
    }
    let mut graph = GraphSnapshot::default();
    graph.events.insert(event.id.clone(), event.clone());
    graph.validate()?;
    Ok(event)
}

fn normalize_artifacts(
    artifacts: Value,
    source_label: &str,
) -> Result<Vec<Value>, AgentProtocolError> {
    let Value::Array(artifacts) = artifacts else {
        return Err(AgentProtocolError::Field("artifacts"));
    };
    artifacts
        .into_iter()
        .map(|artifact| match artifact {
            Value::String(url) => serde_json::to_value(SourceArtifact {
                text: source_label.to_owned(),
                source: source_label.to_owned(),
                url,
            })
            .map_err(AgentProtocolError::from),
            Value::Object(object) => {
                let url = required_string(&object, "url")?.to_owned();
                let text = first_string(&object, &["text", "label", "title"])
                    .unwrap_or("Source")
                    .to_owned();
                let source = object
                    .get("source")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(source_label)
                    .to_owned();
                serde_json::to_value(SourceArtifact { text, source, url })
                    .map_err(AgentProtocolError::from)
            }
            _ => Err(AgentProtocolError::Field("artifacts")),
        })
        .collect()
}

fn parse_bridge(data: Value) -> Result<BridgeUpsert, AgentProtocolError> {
    let object = into_object(data, "bridge.upsert")?;
    let from = EventId::new(required_string(&object, "from")?)?;
    let to = EventId::new(required_string(&object, "to")?)?;
    let label = nonempty(required_string(&object, "label")?, "label")?.to_owned();
    if from == to {
        return Err(AgentProtocolError::Field("to"));
    }
    Ok(BridgeUpsert { from, to, label })
}

fn message(data: Value) -> Result<String, AgentProtocolError> {
    let object = into_object(data, "session")?;
    nonempty(required_string(&object, "message")?, "message").map(str::to_owned)
}

fn into_object(data: Value, kind: &'static str) -> Result<Map<String, Value>, AgentProtocolError> {
    match data {
        Value::Object(object) => Ok(object),
        _ => Err(AgentProtocolError::Object(kind)),
    }
}

fn parse_color(value: Option<&Value>) -> Result<u32, AgentProtocolError> {
    let Some(value) = value else {
        return Err(AgentProtocolError::Field("color"));
    };
    let rejected = || AgentProtocolError::Color(bounded_value(value));
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).map_err(|_| rejected());
    }
    if let Some(number) = value.as_i64() {
        return i32::try_from(number)
            .map(|signed| u32::from_ne_bytes(signed.to_ne_bytes()))
            .map_err(|_| rejected());
    }
    let Some(text) = value.as_str() else {
        return Err(rejected());
    };
    let text = text.trim();
    if text.bytes().all(|byte| byte.is_ascii_digit()) && !matches!(text.len(), 6 | 8) {
        return text.parse::<u32>().map_err(|_| rejected());
    }
    let text = text
        .strip_prefix('#')
        .or_else(|| text.strip_prefix("0x"))
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    let normalized = if text.len() == 6 {
        format!("ff{text}")
    } else {
        text.to_owned()
    };
    if normalized.len() != 8 {
        return Err(rejected());
    }
    u32::from_str_radix(&normalized, 16).map_err(|_| rejected())
}

fn bounded_value(value: &Value) -> String {
    let serialized = value.to_string();
    if serialized.chars().count() <= 64 {
        serialized
    } else {
        serialized.chars().take(64).collect::<String>() + "…"
    }
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a str, AgentProtocolError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| nonempty(value, key).ok())
        .ok_or(AgentProtocolError::Field(key))
}

fn first_string<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
    })
}

fn nonempty<'a>(value: &'a str, field: &'static str) -> Result<&'a str, AgentProtocolError> {
    if value.trim().is_empty() {
        Err(AgentProtocolError::Field(field))
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_and_session_events_remain_typed_progress() {
        assert_eq!(
            parse_output_line("Searching primary sources...").unwrap(),
            AgentEvent::SessionMessage("Searching primary sources...".into())
        );
        assert_eq!(
            parse_output_line(
                r#"AI_NEWS_EVENT: {"type":"session.done","data":{"message":"Complete."}}"#
            )
            .unwrap(),
            AgentEvent::SessionDone("Complete.".into())
        );
    }

    #[test]
    fn event_normalization_preserves_legacy_color_and_artifact_shapes() {
        let AgentEvent::EventUpsert(event) = parse_output_line(
            r##"AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"finding","title":"Finding","date":"Jul 14, 2026","color":"#4cc9d6","summary":"Evidence-backed summary.","sourceLabel":"Primary","artifacts":["https://one.test",{"title":"Document","url":"https://two.test"}]}}"##,
        )
        .unwrap()
        else {
            panic!("expected event")
        };
        assert_eq!(event.color, 0xff4c_c9d6);
        assert_eq!(event.artifacts[0].text, "Primary");
        assert_eq!(event.artifacts[1].text, "Document");
        assert_eq!(event.artifacts[1].source, "Primary");
    }

    #[test]
    fn invalid_or_unknown_mutations_never_become_proposals() {
        for line in [
            r#"AI_NEWS_EVENT: {"type":"unknown","data":{}}"#,
            r#"AI_NEWS_EVENT: {"type":"bridge.upsert","data":{"from":"a","to":"a","label":"same"}}"#,
            r#"AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"","title":"x"}}"#,
            "AI_NEWS_EVENT: not-json",
        ] {
            assert!(parse_output_line(line).is_err(), "accepted {line}");
        }
    }

    #[test]
    fn common_agent_color_spellings_normalize_to_argb() {
        for spelling in ["#4cc9d6", "0xff4cc9d6", "0XFF4CC9D6", "4283222486"] {
            let line = format!(
                r#"AI_NEWS_EVENT: {{"type":"event.upsert","data":{{"id":"finding","title":"Finding","date":"Jul 14, 2026","color":"{spelling}","summary":"Summary","sourceLabel":"Primary","artifacts":[]}}}}"#
            );
            let AgentEvent::EventUpsert(event) = parse_output_line(&line).unwrap() else {
                panic!("expected event")
            };
            assert_eq!(event.color, 0xff4c_c9d6);
        }
        let AgentEvent::EventUpsert(event) = parse_output_line(
            r#"AI_NEWS_EVENT: {"type":"event.upsert","data":{"id":"finding","title":"Finding","date":"Jul 14, 2026","color":-11744810,"summary":"Summary","sourceLabel":"Primary","artifacts":[]}}"#,
        )
        .unwrap()
        else {
            panic!("expected event")
        };
        assert_eq!(
            event.color,
            u32::from_ne_bytes((-11_744_810_i32).to_ne_bytes())
        );
    }
}
