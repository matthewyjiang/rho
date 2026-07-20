use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{AliasTarget, ModelAliasResolutionError, ModelAliases, ResolvedModelReference};

fn entries(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn parses_provider_qualified_bare_and_slash_qualified_targets() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("fast", "gpt-5.5"),
        ("open", "openrouter/anthropic/claude-sonnet-4"),
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
    assert_eq!(
        aliases.get("open"),
        Some(&AliasTarget {
            provider: Some("openrouter".into()),
            model: "anthropic/claude-sonnet-4".into(),
        })
    );
    assert_eq!(aliases.get("missing"), None);
}

#[test]
fn resolves_explicit_aliases_with_provenance() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("fast", "gpt-5.5"),
    ]))
    .unwrap();

    assert_eq!(
        aliases.resolve("@deep"),
        Ok(ResolvedModelReference {
            alias: Some("deep".into()),
            provider: Some("anthropic".into()),
            model: "claude-opus-4-8".into(),
        })
    );
    assert_eq!(
        aliases.resolve("@fast"),
        Ok(ResolvedModelReference {
            alias: Some("fast".into()),
            provider: None,
            model: "gpt-5.5".into(),
        })
    );
}

#[test]
fn undefined_explicit_alias_returns_typed_actionable_error() {
    let aliases = ModelAliases::default();
    let error = aliases.resolve("@missing").unwrap_err();

    assert_eq!(
        error,
        ModelAliasResolutionError::UndefinedAlias {
            name: "missing".into(),
        }
    );
    assert_eq!(
        error.to_string(),
        "model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
    );
}

#[test]
fn ordinary_model_reference_is_concrete_even_when_it_shadows_an_alias() {
    let aliases =
        ModelAliases::from_entries(entries(&[("deep", "anthropic/claude-opus-4-8")])).unwrap();

    assert_eq!(
        aliases.resolve("deep"),
        Ok(ResolvedModelReference {
            alias: None,
            provider: None,
            model: "deep".into(),
        })
    );
}

#[test]
fn ordinary_provider_qualified_reference_remains_concrete_for_the_caller() {
    let aliases = ModelAliases::default();

    assert_eq!(
        aliases.resolve("openrouter/anthropic/claude-sonnet-4"),
        Ok(ResolvedModelReference {
            alias: None,
            provider: None,
            model: "openrouter/anthropic/claude-sonnet-4".into(),
        })
    );
}

#[test]
fn concrete_targets_may_share_alias_names_without_becoming_chains() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("deeper", "deep"),
        ("qualified", "openrouter/deep"),
    ]))
    .unwrap();

    assert_eq!(
        aliases.resolve("@deeper"),
        Ok(ResolvedModelReference {
            alias: Some("deeper".into()),
            provider: None,
            model: "deep".into(),
        })
    );
    assert_eq!(
        aliases.resolve("@qualified"),
        Ok(ResolvedModelReference {
            alias: Some("qualified".into()),
            provider: Some("openrouter".into()),
            model: "deep".into(),
        })
    );
}

#[test]
fn rejects_malformed_names_and_values() {
    for (name, value) in [
        ("bad name", "gpt-5.5"),
        ("bad/name", "gpt-5.5"),
        ("bad@name", "gpt-5.5"),
        ("", "gpt-5.5"),
        ("deep", ""),
        ("deep", "anthropic/"),
        ("deep", "/claude-opus-4-8"),
        ("deep", "@other"),
        ("deep", "claude opus"),
    ] {
        let error = ModelAliases::from_entries(entries(&[(name, value)])).unwrap_err();
        assert!(error.contains("model alias"), "{name}={value}: {error}");
    }
}

#[test]
fn alias_values_must_not_use_alias_reference_syntax() {
    let error = ModelAliases::from_entries(entries(&[("deep", "@other")])).unwrap_err();

    assert_eq!(
        error,
        "invalid model alias value '@other': expected a concrete 'provider/model' or 'model' with no whitespace"
    );
}

#[test]
fn serialization_preserves_concrete_targets_in_a_round_trip() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("fast", "gpt-5.5"),
        ("open", "openrouter/anthropic/claude-sonnet-4"),
    ]))
    .unwrap();

    let toml = toml::to_string(&aliases).unwrap();
    assert_eq!(
        toml,
        "deep = \"anthropic/claude-opus-4-8\"\nfast = \"gpt-5.5\"\nopen = \"openrouter/anthropic/claude-sonnet-4\"\n"
    );
    let round_tripped: ModelAliases = toml::from_str(&toml).unwrap();
    assert_eq!(round_tripped, aliases);
}

#[test]
fn deserialization_rejects_alias_reference_values() {
    let error = toml::from_str::<ModelAliases>("deep = \"@other\"\n").unwrap_err();

    assert!(
        error
            .to_string()
            .contains("invalid model alias value '@other'"),
        "{error}"
    );
}
