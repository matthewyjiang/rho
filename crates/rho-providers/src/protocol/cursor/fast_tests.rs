use pretty_assertions::assert_eq;

use crate::protocol::cursor::effort::CursorEffort;
use crate::reasoning::ReasoningLevel;

use super::{catalog_model_id, supports_fast_mode, wire_model_id, CursorSpeed};

// Covers: Fast is a trailing -fast suffix; Auto and product names like grok-code-fast-1 stay put
// Owner: cursor protocol
#[test]
fn cursor_fast_ids_follow_trailing_suffix_rules() {
    let cases = [
        (
            "auto",
            CursorSpeed::Standard,
            CursorEffort::Unspecified,
            "default",
            false,
            "auto",
        ),
        (
            "auto",
            CursorSpeed::Fast,
            CursorEffort::Unspecified,
            "default",
            false,
            "auto",
        ),
        (
            "grok-4.6-high",
            CursorSpeed::Standard,
            CursorEffort::Unspecified,
            "grok-4.6-high",
            true,
            "grok-4.6",
        ),
        (
            "grok-4.6-high",
            CursorSpeed::Fast,
            CursorEffort::Unspecified,
            "grok-4.6-high-fast",
            true,
            "grok-4.6",
        ),
        (
            "grok-4.6-high-fast",
            CursorSpeed::Standard,
            CursorEffort::Unspecified,
            "grok-4.6-high",
            true,
            "grok-4.6",
        ),
        (
            "grok-4.6-high-fast",
            CursorSpeed::Fast,
            CursorEffort::Unspecified,
            "grok-4.6-high-fast",
            true,
            "grok-4.6",
        ),
        (
            "grok-4.6",
            CursorSpeed::Fast,
            CursorEffort::Level(ReasoningLevel::Xhigh),
            "grok-4.6-xhigh-fast",
            true,
            "grok-4.6",
        ),
        (
            "grok-4.6-xhigh-fast",
            CursorSpeed::Standard,
            CursorEffort::Unspecified,
            "grok-4.6-xhigh",
            true,
            "grok-4.6",
        ),
        (
            "composer-2-fast",
            CursorSpeed::Fast,
            CursorEffort::Unspecified,
            "composer-2-fast",
            true,
            "composer-2",
        ),
        (
            "grok-code-fast-1",
            CursorSpeed::Fast,
            CursorEffort::Unspecified,
            "grok-code-fast-1",
            false,
            "grok-code-fast-1",
        ),
    ];

    for (model, speed, effort, wire, supported, catalog) in cases {
        assert_eq!(
            (
                wire_model_id(model, speed, effort),
                supports_fast_mode(model),
                catalog_model_id(model)
            ),
            (wire.to_string(), supported, catalog),
            "model={model} speed={speed:?} effort={effort:?}"
        );
    }
}
