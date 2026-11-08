use pretty_assertions::assert_eq;

use super::{collapse, ListedModel};
use crate::model::{provider_models::ProviderModel, ReasoningCapabilities};

fn listed(id: &str, aliases: &[&str]) -> ListedModel {
    ListedModel {
        model: ProviderModel {
            provider: "host".into(),
            model: id.into(),
            display_name: format!("{id} display"),
            context_window: None,
            max_output_tokens: None,
            reasoning_capabilities: ReasoningCapabilities::Unknown,
        },
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
    }
}

// Covers: aliases join entries transitively (a chain where no entry names
// every other), keep aliases the host did not list on their own (xAI's
// `grok-4-latest`), and never merge distinct models
// Owner: provider model alias collapsing
#[test]
fn collapse_groups_transitive_aliases_and_keeps_unlisted_ones() {
    let records = collapse(
        vec![
            listed("a-2604", &["a-latest"]),
            listed("b", &["b-latest"]),
            listed("a-latest", &["a"]),
            listed("a", &[]),
        ],
        |_| false,
    );

    let summary = records
        .iter()
        .map(|record| (record.model.model.as_str(), record.aliases.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        [
            ("a-2604", vec!["a".to_owned(), "a-latest".to_owned()]),
            ("b", vec!["b-latest".to_owned()]),
        ]
    );
}
