use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

/// User-defined model aliases: short names mapped to concrete models.
///
/// An alias value is either `provider/model` or a bare `model` id (which
/// keeps whichever provider is otherwise selected). Aliases are consulted
/// wherever a model can be referenced — the session model, `--model`, and
/// agent `model:` frontmatter — and an alias always wins over an identically
/// named model id, so a provider release can never silently change what a
/// configured name points to. Resolution is a single flat lookup performed
/// before any model-specific behavior; downstream code only ever sees
/// concrete model ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelAliases(BTreeMap<String, AliasTarget>);

/// The concrete model an alias resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasTarget {
    /// Provider to switch to, or `None` to keep the current provider.
    pub provider: Option<String>,
    pub model: String,
}

impl fmt::Display for AliasTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider {
            Some(provider) => write!(formatter, "{provider}/{}", self.model),
            None => formatter.write_str(&self.model),
        }
    }
}

impl ModelAliases {
    pub fn from_entries(entries: BTreeMap<String, String>) -> Result<Self, String> {
        let mut aliases = BTreeMap::new();
        for (name, value) in entries {
            if name.is_empty() || name.contains(char::is_whitespace) || name.contains('/') {
                return Err(format!(
                    "invalid model alias name '{name}': must be non-empty with no whitespace or '/'"
                ));
            }
            aliases.insert(name, parse_target(&value)?);
        }
        for (name, target) in &aliases {
            if aliases.contains_key(&target.model) {
                return Err(format!(
                    "model alias '{name}' targets alias '{}'; alias values must be concrete models",
                    target.model
                ));
            }
        }
        Ok(Self(aliases))
    }

    pub fn get(&self, name: &str) -> Option<&AliasTarget> {
        self.0.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn parse_target(value: &str) -> Result<AliasTarget, String> {
    let invalid = |value: &str| {
        format!("invalid model alias value '{value}': expected 'provider/model' or 'model' with no whitespace")
    };
    if value.is_empty() || value.contains(char::is_whitespace) {
        return Err(invalid(value));
    }
    let (provider, model) = match value.split_once('/') {
        Some((provider, model)) => (Some(provider), model),
        None => (None, value),
    };
    if provider.is_some_and(str::is_empty) || model.is_empty() || model.contains('/') {
        return Err(invalid(value));
    }
    Ok(AliasTarget {
        provider: provider.map(str::to_string),
        model: model.to_string(),
    })
}

impl Serialize for ModelAliases {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(
            self.0
                .iter()
                .map(|(name, target)| (name, target.to_string())),
        )
    }
}

impl<'de> Deserialize<'de> for ModelAliases {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let entries = BTreeMap::<String, String>::deserialize(deserializer)?;
        Self::from_entries(entries).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "model_aliases_tests.rs"]
mod tests;
