use pretty_assertions::assert_eq;

use super::*;
use crate::claude_runtime::stream::{StatusPatch, StreamEffect};

fn init_effect(model: &str) -> StreamEffect {
    StreamEffect::Status(StatusPatch {
        claude_model: Some(model.into()),
        ..StatusPatch::default()
    })
}

// Covers: the store must key each `--model` value separately and keep only what
// a run actually reported. Collapsing `opus` with the unpinned default, or
// storing a blank report, would name the wrong model in prompt text.
// Owner: Claude alias resolution.
#[test]
fn each_requested_value_keeps_the_last_model_a_run_reported_for_it() {
    let _guard = test_lock();
    clear_for_tests();

    // (requested, reported) pairs applied in order.
    let reports = [
        (Some("opus"), "claude-opus-4-6"),
        (Some("sonnet"), "claude-sonnet-5"),
        (None, "claude-sonnet-5"),
        // A newer run replaces what an alias points at.
        (Some("opus"), "claude-opus-5"),
        // Blank and whitespace-only reports are not reports.
        (Some("haiku"), "   "),
        // Surrounding whitespace is not part of the model id.
        (Some("fable"), "  claude-fable-5\n"),
    ];
    for (requested, reported) in reports {
        note_stream_effect(requested, &init_effect(reported));
    }
    // An effect carrying no model leaves the entry alone.
    note_stream_effect(Some("opus"), &StreamEffect::Status(StatusPatch::default()));

    let expected = [
        (Some("opus"), Some("claude-opus-5")),
        (Some("sonnet"), Some("claude-sonnet-5")),
        (None, Some("claude-sonnet-5")),
        (Some("haiku"), None),
        (Some("fable"), Some("claude-fable-5")),
        // Never reported at all.
        (Some("gpt-5"), None),
    ];
    for (requested, model) in expected {
        assert_eq!(
            last_resolved(requested).as_deref(),
            model,
            "requested {requested:?}"
        );
    }
}
