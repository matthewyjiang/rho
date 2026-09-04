use pretty_assertions::assert_eq;
use rho_sdk::model::handoff::HandoffReport;

use super::{confirm_send_choice, ACTION_COMPACT_SEND, ACTION_DONT_SEND, ACTION_SEND};

fn omissions(count: usize) -> HandoffReport {
    HandoffReport {
        omitted_provider_context: count,
        omitted_kinds: if count == 0 {
            Vec::new()
        } else {
            vec!["openai_response_output_item".into()]
        },
    }
}

// Covers: the confirm-send modal always offers send/don't-send and only offers
// compaction when the conversation can be compacted
// Owner: send confirm gate
#[test]
fn confirm_send_options_depend_on_compact_availability() {
    let compactable = confirm_send_choice("xai/grok-4", &omissions(115), true).unwrap();
    assert_eq!(
        compactable
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec![ACTION_SEND, ACTION_COMPACT_SEND, ACTION_DONT_SEND]
    );

    let not_compactable = confirm_send_choice("xai/grok-4", &omissions(115), false).unwrap();
    assert_eq!(
        not_compactable
            .options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        vec![ACTION_SEND, ACTION_DONT_SEND]
    );
}
