use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{AliasTarget, ModelAliases};

fn entries(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn parses_provider_qualified_and_bare_targets() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("fast", "gpt-5.5"),
    ]))
    .unwrap();

    assert_eq!(
        aliases.get("deep"),
        Some(&AliasTarget {
            provider: Some("anthropic".into()),
            model: "claude-opus-4-8".into(),
        })
    );
    assert_eq!(
        aliases.get("fast"),
        Some(&AliasTarget {
            provider: None,
            model: "gpt-5.5".into(),
        })
    );
    assert_eq!(aliases.get("missing"), None);
}

#[test]
fn rejects_malformed_names_and_values() {
    for (name, value) in [
        ("bad name", "gpt-5.5"),
        ("bad/name", "gpt-5.5"),
        ("", "gpt-5.5"),
        ("deep", ""),
        ("deep", "anthropic/"),
        ("deep", "/claude-opus-4-8"),
        ("deep", "a/b/c"),
        ("deep", "claude opus"),
    ] {
        let error = ModelAliases::from_entries(entries(&[(name, value)])).unwrap_err();
        assert!(error.contains("model alias"), "{name}={value}: {error}");
    }
}

#[test]
fn rejects_alias_targeting_another_alias() {
    let error = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("deeper", "deep"),
    ]))
    .unwrap_err();

    assert_eq!(
        error,
        "model alias 'deeper' targets alias 'deep'; alias values must be concrete models"
    );
}

#[test]
fn serializes_targets_as_reference_strings() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("fast", "gpt-5.5"),
    ]))
    .unwrap();

    let toml = toml::to_string(&aliases).unwrap();
    let round_tripped: ModelAliases = toml::from_str(&toml).unwrap();
    assert_eq!(round_tripped, aliases);
}
