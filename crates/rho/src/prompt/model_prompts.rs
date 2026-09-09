//! Strict, global model prompt catalog. Filenames do not determine identity.

use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::model_identity::PromptModel;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ModelPromptMode {
    #[default]
    Append,
    Replace,
}

impl ModelPromptMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Replace => "replace",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelPrompt {
    pub(crate) path: PathBuf,
    pub(crate) mode: ModelPromptMode,
    pub(crate) body: String,
    /// Lowercase SHA-256 of the complete file, including frontmatter.
    pub(crate) sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Frontmatter {
    #[serde(deserialize_with = "deserialize_identifier")]
    pub(super) provider: String,
    #[serde(deserialize_with = "deserialize_identifier")]
    pub(super) model: String,
    #[serde(default)]
    mode: ModelPromptMode,
}

fn deserialize_identifier<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let value = serde_yaml_ng::Value::deserialize(deserializer)?;
    match value {
        serde_yaml_ng::Value::String(value)
            if !value.is_empty() && !value.chars().any(char::is_whitespace) =>
        {
            Ok(value)
        }
        _ => Err(serde::de::Error::custom(
            "expected a nonempty exact identifier without whitespace",
        )),
    }
}

/// Validate the entire top-level catalog before selecting an exact provider/model match.
/// External CLI runtimes own their prompts and never read this catalog.
pub(crate) fn load(home: Option<&Path>, model: &PromptModel) -> Result<Option<ModelPrompt>> {
    let (provider, model) = match model {
        PromptModel::Rho { provider, model } => (provider, model),
        PromptModel::ExternalCli { .. } => return Ok(None),
    };
    let Some(home) = home else {
        return Ok(None);
    };
    let directory = home.join(".rho/model-prompts");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "could not read model prompt directory {}",
                    directory.display()
                )
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "could not read entry in model prompt directory {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let metadata = fs::metadata(&path)
            .with_context(|| format!("could not inspect model prompt {}", path.display()))?;
        if metadata.is_file() {
            paths.push(path);
        }
    }
    // Stable order makes duplicate and malformed-file errors deterministic.
    paths.sort();
    let mut identities = BTreeMap::new();
    let mut selected = None;
    for path in paths {
        let source = fs::read_to_string(&path)
            .with_context(|| format!("could not read model prompt {}", path.display()))?;
        let (frontmatter, prompt) = parse(&path, &source)
            .with_context(|| format!("invalid model prompt {}", path.display()))?;
        let identity = (frontmatter.provider, frontmatter.model);
        if let Some(previous) = identities.insert(identity.clone(), path.clone()) {
            bail!(
                "duplicate model prompt for {}/{} in {} and {}",
                identity.0,
                identity.1,
                previous.display(),
                path.display()
            );
        }
        if identity.0 == *provider && identity.1 == *model {
            selected = Some(prompt);
        }
    }
    Ok(selected)
}

pub(super) fn parse(path: &Path, source: &str) -> Result<(Frontmatter, ModelPrompt)> {
    let mut lines = source.split_inclusive('\n');
    if lines.next().map(|line| line.trim_end_matches(['\r', '\n'])) != Some("---") {
        bail!("required YAML frontmatter must begin with --- on the first line");
    }
    let start = source
        .find('\n')
        .context("missing closing YAML frontmatter delimiter")?
        + 1;
    let mut end = start;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let frontmatter = serde_yaml_ng::from_str::<Frontmatter>(&source[start..end])
                .context("invalid YAML frontmatter")?;
            let body = &source[end + line.len()..];
            if body.trim().is_empty() {
                bail!("model prompt body must not be empty");
            }
            let mode = frontmatter.mode;
            return Ok((
                frontmatter,
                ModelPrompt {
                    path: path.to_owned(),
                    mode,
                    body: body.to_owned(),
                    sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
                },
            ));
        }
        end += line.len();
    }
    bail!("missing closing YAML frontmatter delimiter")
}

#[cfg(test)]
#[path = "model_prompts_tests.rs"]
mod tests;
