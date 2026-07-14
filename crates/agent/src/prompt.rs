use std::fmt::Write as _;

use not_news_domain::GraphSnapshot;

const SOURCE_POLICY: &str = r"Use direct primary sources whenever possible. Use the available
semantic web search, a configured SearXNG endpoint, and Browse.sh as complementary
discovery/retrieval surfaces when external research needs them. Compare their
candidate sets instead of letting one dominate. Inspect unresponsive search
engines and retry weak results with source-qualified queries, broader categories,
or an appropriate time range. Use browser automation for JavaScript-heavy,
dynamically rendered, search-gated, or thinly extracted candidates. Treat search
snippets and extraction output as leads, not final evidence. Prefer official
documentation, releases, papers, filings, standards, repositories, and original
announcements for final claims. Never expose credentials in output.";

/// Builds the provider-neutral research contract and a bounded identity digest
/// of the graph that new findings must join.
pub fn build_research_prompt(question: &str, graph: &GraphSnapshot) -> String {
    let mut context = String::new();
    for event in graph.events.values() {
        if context.len() >= 16_000 {
            context.push_str("- Additional saved events omitted from the prompt digest.\n");
            break;
        }
        let primary = event.url.as_deref().unwrap_or("no-primary-url");
        writeln!(
            context,
            "- id={:?}; title={:?}; date={:?}; primary={:?}",
            bounded(&event.id.0, 256),
            bounded(&event.title, 240),
            bounded(&event.date, 80),
            bounded(primary, 1_024)
        )
        .expect("writing to String is infallible");
    }
    if context.is_empty() {
        context.push_str("- The Canvas is empty; form a connected new cluster.\n");
    }

    format!(
        r#"Research the question below for an event-graph Canvas. Work only in the supplied
scratch directory; do not modify application source, user documents, or the saved
Canvas directly. Report concise, evidence-backed events, source artifacts, and
relationships. Emit every mutation on its own line, with no Markdown fence:
AI_NEWS_EVENT: {{"type":"event.upsert","data":{{...}}}}
AI_NEWS_EVENT: {{"type":"bridge.upsert","data":{{...}}}}

Event keys are exactly id, title, date, color, summary, sourceLabel, artifacts,
url. Every artifact uses text, source, url. Every bridge uses from, to, label;
its endpoints must exactly match an existing ID listed below or an event ID you
already emitted. Prefer integer ARGB colors. A URL may occur only once across all
new event and artifact URLs. If one source supports several subfindings, combine
them or find distinct primary URLs. Connect every semantically related new event;
emit an isolated singleton only when it is truly unrelated and explain why in a
session.message. You may emit sparse voice.note orientation under 110 characters.
Finish with AI_NEWS_EVENT: {{"type":"session.done","data":{{"message":"..."}}}}.

{SOURCE_POLICY}

Existing Canvas identity digest (reference only; never rewrite it):
{context}
Question: {}"#,
        bounded(question.trim(), 4_096)
    )
}

fn bounded(value: &str, limit: usize) -> String {
    let mut characters = value.chars();
    let mut bounded: String = characters.by_ref().take(limit).collect();
    if characters.next().is_some() {
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use not_news_domain::{EventId, ResearchEvent};

    use super::*;

    #[test]
    fn prompt_exposes_identity_without_serializing_summaries_or_credentials() {
        let id = EventId("event-a".into());
        let graph = GraphSnapshot {
            events: IndexMap::from([(
                id.clone(),
                ResearchEvent {
                    id,
                    title: "A finding".into(),
                    date: "Jul 14, 2026".into(),
                    color: 0xff00_0000,
                    summary: "private long-form workspace note".into(),
                    source_label: "Primary".into(),
                    artifacts: vec![],
                    url: Some("https://example.test/a".into()),
                },
            )]),
            ..GraphSnapshot::default()
        };
        let prompt = build_research_prompt("What changed?", &graph);
        assert!(prompt.contains("event-a"));
        assert!(prompt.contains("https://example.test/a"));
        assert!(prompt.contains("What changed?"));
        assert!(!prompt.contains("private long-form workspace note"));
    }

    #[test]
    fn prompt_bounds_untrusted_question_and_identity_fields() {
        let prompt = build_research_prompt(&"q".repeat(8_000), &GraphSnapshot::default());
        assert!(prompt.len() < 10_000);
        assert!(prompt.contains(&"q".repeat(4_096)));
        assert!(!prompt.contains(&"q".repeat(4_097)));
    }
}
