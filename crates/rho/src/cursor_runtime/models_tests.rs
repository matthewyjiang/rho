use pretty_assertions::assert_eq;
use rho_providers::model::provider_models::{
    with_provider_models_cache_dir_for_tests, CliProviderRefreshContext,
};

use super::*;

const LIVE_MODELS: &str = include_str!("fixtures/live_models.txt");

fn by_id<'a>(models: &'a [CursorModel], id: &str) -> &'a CursorModel {
    models
        .iter()
        .find(|model| model.id == id)
        .unwrap_or_else(|| panic!("missing model {id}"))
}

// Covers: cursor-agent models plain text (headers, tip, flags, combined
// annotations) must parse into 1:1 Cursor ids.
// Owner: cursor model list parse
#[test]
fn parses_live_models_fixture_and_annotation_edges() {
    let models = parse_models_output(LIVE_MODELS).expect("fixture must parse");
    assert_eq!(models.len(), 217);

    assert_eq!(
        by_id(&models, "auto"),
        &CursorModel {
            id: "auto".into(),
            display_name: "Auto".into(),
            is_default: true,
            is_current: false,
            zdr: true,
        }
    );
    assert_eq!(
        by_id(&models, "composer-2.5"),
        &CursorModel {
            id: "composer-2.5".into(),
            display_name: "Composer 2.5".into(),
            is_default: false,
            is_current: true,
            zdr: true,
        }
    );
    assert_eq!(
        by_id(&models, "claude-fable-5-thinking-high"),
        &CursorModel {
            id: "claude-fable-5-thinking-high".into(),
            display_name: "Claude Fable 5 1M Thinking".into(),
            is_default: false,
            is_current: false,
            zdr: false,
        }
    );
    assert_eq!(
        by_id(&models, "gpt-5.3-codex-high-fast").display_name,
        "Codex 5.3 High Fast"
    );

    let two = parse_models_output(
        "sample - Sample Name (default) (NO ZDR)\nunknown - Keeps Tail (mystery)\n",
    )
    .unwrap();
    assert_eq!(
        two,
        vec![
            CursorModel {
                id: "sample".into(),
                display_name: "Sample Name".into(),
                is_default: true,
                is_current: false,
                zdr: false,
            },
            CursorModel {
                id: "unknown".into(),
                display_name: "Keeps Tail (mystery)".into(),
                is_default: false,
                is_current: false,
                zdr: true,
            },
        ]
    );

    assert!(matches!(
        parse_models_output("Available models\n\nTip: use --model\n"),
        Err(CursorModelsError::EmptyList)
    ));
}

// Covers: picker families come from display names by stripping trailing
// effort/speed/thinking tokens, not from collapsing Cursor ids.
// Owner: cursor model list parse
#[test]
fn display_family_strips_trailing_effort_tokens() {
    let cases = [
        ("Claude Opus 5 1M Extra High Thinking Fast", "Claude Opus 5"),
        ("Codex 5.3 High Fast", "Codex 5.3"),
        ("Claude Fable 5 1M Thinking", "Claude Fable 5"),
        ("Composer 2.5", "Composer 2.5"),
        ("Auto", "Auto"),
        ("GLM 5.2 Max", "GLM 5.2"),
        ("Kimi K3", "Kimi K3"),
    ];
    for (display_name, family) in cases {
        let model = CursorModel {
            id: "id".into(),
            display_name: display_name.into(),
            is_default: false,
            is_current: false,
            zdr: true,
        };
        assert_eq!(model.display_family(), family, "{display_name}");
    }
}

// Covers: cache decode must restore default/current/zdr flags from raw_json.
// Owner: cursor model list parse
#[test]
fn cached_models_restore_raw_json_flags() {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        let models = vec![
            CursorModel {
                id: "auto".into(),
                display_name: "Auto".into(),
                is_default: true,
                is_current: false,
                zdr: true,
            },
            CursorModel {
                id: "claude-fable-5-thinking-high".into(),
                display_name: "Claude Fable 5 1M Thinking".into(),
                is_default: false,
                is_current: false,
                zdr: false,
            },
        ];
        cache_models(
            &models,
            CliProviderRefreshContext {
                account_email: Some("dev@example.com".into()),
                cursor_version: Some("2026.09.02".into()),
            },
        )
        .unwrap();
        assert_eq!(cached(), models);
        assert!(!needs_refresh());
        assert!(!needs_refresh_for_account(Some("dev@example.com")));
        assert!(needs_refresh_for_account(Some("other@example.com")));
    });
}
