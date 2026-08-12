use pretty_assertions::assert_eq;

use super::{catalog_model_id, supports_fast_mode, wire_model_id, CursorSpeed};

// Covers: Fast is a trailing -fast suffix; Auto and product names like grok-code-fast-1 stay put
// Owner: cursor protocol
#[test]
fn cursor_fast_ids_follow_trailing_suffix_rules() {
    let cases = [
        ("auto", CursorSpeed::Standard, "default", false, "auto"),
        ("auto", CursorSpeed::Fast, "default", false, "auto"),
        (
            "grok-4.6-high",
            CursorSpeed::Standard,
            "grok-4.6-high",
            true,
            "grok-4.6-high",
        ),
        (
            "grok-4.6-high",
            CursorSpeed::Fast,
            "grok-4.6-high-fast",
            true,
            "grok-4.6-high",
        ),
        (
            "grok-4.6-high-fast",
            CursorSpeed::Standard,
            "grok-4.6-high",
            true,
            "grok-4.6-high",
        ),
        (
            "grok-4.6-high-fast",
            CursorSpeed::Fast,
            "grok-4.6-high-fast",
            true,
            "grok-4.6-high",
        ),
        (
            "composer-2-fast",
            CursorSpeed::Fast,
            "composer-2-fast",
            true,
            "composer-2",
        ),
        (
            "grok-code-fast-1",
            CursorSpeed::Fast,
            "grok-code-fast-1",
            false,
            "grok-code-fast-1",
        ),
    ];

    for (model, speed, wire, supported, catalog) in cases {
        assert_eq!(
            (
                wire_model_id(model, speed),
                supports_fast_mode(model),
                catalog_model_id(model)
            ),
            (wire.to_string(), supported, catalog),
            "model={model} speed={speed:?}"
        );
    }
}
