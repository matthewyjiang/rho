use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

/// User-defined model aliases: short names mapped to concrete models.
///
/// Alias references use the explicit `@name` syntax. All other references are
/// concrete model targets, even when their model id happens to match an alias
/// name. An alias value is either `provider/model` or a bare `model` id (which
/// keeps whichever provider is otherwise selected). Provider-qualified model
/// ids may themselves contain slashes; only the first slash separates the
/// provider from the model id.
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

/// A concrete model reference produced by [`ModelAliases::resolve`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModelReference {
    /// The alias name, without its leading `@`, when the input was an alias.
    pub alias: Option<String>,
    /// Provider selected by a qualified reference, or `None` to keep the
    /// provider selected by the caller.
    pub provider: Option<String>,
    pub model: String,
}

/// An error resolving a model alias reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelAliasResolutionError {
    UndefinedAlias { name: String },
}

impl fmt::Display for ModelAliasResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedAlias { name } => write!(
                formatter,
                "model alias '@{name}' is not defined; define it in [model.aliases] or use a concrete model reference"
            ),
        }
    }
}

impl std::error::Error for ModelAliasResolutionError {}

impl ModelAliases {
    pub fn from_entries(entries: BTreeMap<String, String>) -> Result<Self, String> {
        let mut aliases = BTreeMap::new();
        for (name, value) in entries {
            if name.is_empty() || name.contains(char::is_whitespace) || name.contains(['/', '@']) {
                return Err(format!(
                    "invalid model alias name '{name}': must be non-empty with no whitespace, '/', or '@'"
                ));
            }
            aliases.insert(name, parse_target(&value)?);
        }
        Ok(Self(aliases))
    }

    /// Resolves an explicit `@name` alias or returns an ordinary concrete model
    /// reference unchanged. Ordinary references never consult the alias table.
    pub fn resolve(
        &self,
        reference: &str,
    ) -> Result<ResolvedModelReference, ModelAliasResolutionError> {
        if let Some(name) = reference.strip_prefix('@') {
            let target =
                self.0
                    .get(name)
                    .ok_or_else(|| ModelAliasResolutionError::UndefinedAlias {
                        name: name.to_string(),
                    })?;
            return Ok(ResolvedModelReference {
                alias: Some(name.to_string()),
                provider: target.provider.clone(),
                model: target.model.clone(),
            });
        }

        Ok(ResolvedModelReference {
            alias: None,
            provider: None,
            model: reference.to_string(),
        })
    }

    pub fn get(&self, name: &str) -> Option<&AliasTarget> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &AliasTarget)> {
        self.0.iter().map(|(name, target)| (name.as_str(), target))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn split_target(value: &str) -> (Option<&str>, &str) {
    match value.split_once('/') {
        Some((provider, model)) => (Some(provider), model),
        None => (None, value),
    }
}

fn parse_target(value: &str) -> Result<AliasTarget, String> {
    let invalid = |value: &str| {
        format!(
            "invalid model alias value '{value}': expected a concrete 'provider/model' or 'model' with no whitespace"
        )
    };
    if value.is_empty() || value.starts_with('@') || value.contains(char::is_whitespace) {
        return Err(invalid(value));
    }
    let (provider, model) = split_target(value);
    if provider.is_some_and(str::is_empty) || model.is_empty() {
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
