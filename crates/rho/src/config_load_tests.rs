use super::{collect_unknown_keys, parse_settings, ConfigWarning};
use pretty_assertions::assert_eq;

// Covers: unknown top-level config keys must surface as warnings
// Owner: config load
#[test]
fn unknown_top_level_keys_are_reported() {
    let raw = toml::from_str("provder = \"openai\"\nmodel = \"gpt-5.5\"\n").unwrap();
    assert_eq!(
        collect_unknown_keys(&raw),
        vec![ConfigWarning::UnknownKey {
            path: "provder".into(),
        }]
    );
}

// Covers: unknown nested config keys must surface with a dotted path
// Owner: config load
#[test]
fn unknown_nested_keys_are_reported() {
    let raw = toml::from_str(
        r#"
[display]
max_tool_output_liness = 3
"#,
    )
    .unwrap();
    assert_eq!(
        collect_unknown_keys(&raw),
        vec![ConfigWarning::UnknownKey {
            path: "display.max_tool_output_liness".into(),
        }]
    );
}

// Covers: max_tool_output_lines below 1 clamps with a warning
// Owner: config load
#[test]
fn max_tool_output_lines_clamp_warns() {
    let (config, warnings) = parse_settings(
        r#"
[display]
max_tool_output_lines = 0
"#,
    )
    .unwrap();

    assert_eq!(config.max_tool_output_lines, 1);
    assert_eq!(
        warnings,
        vec![ConfigWarning::Clamped {
            key: "display.max_tool_output_lines",
            from: "0".into(),
            to: "1".into(),
        }]
    );
}

// Covers: unsupported web_search.provider normalizes to auto with a warning
// Owner: config load
#[test]
fn unsupported_web_search_provider_normalizes_with_warning() {
    let (config, warnings) = parse_settings(
        r#"
[web_search]
provider = "unknown"
"#,
    )
    .unwrap();

    assert_eq!(
        config.web_search_provider,
        super::super::SearchProvider::Auto
    );
    assert_eq!(
        warnings,
        vec![ConfigWarning::Normalized {
            key: "web_search.provider",
            from: "\"unknown\"".into(),
            to: "\"auto\"".into(),
        }]
    );
}

// Covers: known keys alone produce no warnings
// Owner: config load
#[test]
fn known_config_produces_no_warnings() {
    let (_config, warnings) = parse_settings(
        r#"
[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
"#,
    )
    .unwrap();
    assert_eq!(warnings, Vec::<ConfigWarning>::new());
}
