use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::{ModelAliasResolutionError, ModelAliases, ResolvedModelReference};

fn entries(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn resolves_explicit_aliases_with_provenance() {
    let aliases = ModelAliases::from_entries(entries(&[
        ("deep", "anthropic/claude-opus-4-8"),
        ("fast", "gpt-5.5"),
        ("open", "openrouter/anthropic/claude-sonnet-4"),
    ]))
    .unwrap();

    for (reference, alias, provider, model) in [
        ("@deep", "deep", Some("anthropic"), "claude-opus-4-8"),
        ("@fast", "fast", None, "gpt-5.5"),
        (
            "@open",
            "open",
            Some("openrouter"),
            "anthropic/claude-sonnet-4",
        ),
    ] {
        assert_eq!(
            aliases.resolve(reference),
            Ok(ResolvedModelReference {
                alias: Some(alias.into()),
                provider: provider.map(Into::into),
                model: model.into(),
            }),
            "{reference}"
        );
    }
}

#[test]
fn undefined_explicit_alias_returns_typed_error() {
    let aliases = ModelAliases::default();

    assert_eq!(
        aliases.resolve("@missing"),
        Err(ModelAliasResolutionError::UndefinedAlias {
            name: "missing".into(),
        })
    );
}

#[test]
fn ordinary_model_references_stay_concrete_without_alias_resolution() {
    let aliases =
        ModelAliases::from_entries(entries(&[("deep", "anthropic/claude-opus-4-8")])).unwrap();

    for (input, model) in [
        ("deep", "deep"),
        (
            "openrouter/anthropic/claude-sonnet-4",
            "openrouter/anthropic/claude-sonnet-4",
        ),
    ] {
        assert_eq!(
            aliases.resolve(input),
            Ok(ResolvedModelReference {
                alias: None,
                provider: None,
                model: model.into(),
            }),
            "{input}"
        );
    }
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
