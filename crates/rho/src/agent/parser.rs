use std::{collections::BTreeSet, path::Path};

use crate::reasoning::ReasoningLevel;

use super::{
    catalog::AgentCatalogError,
    definition::{
        AgentDefinition, AgentId, ModelPolicy, ModelSelection, PromptPolicy, ToolPolicy,
        KNOWN_TOOLS,
    },
};

const MAX_DESCRIPTION_LEN: usize = 1024;

#[derive(Default)]
struct RawDefinition {
    id: Option<String>,
    description: Option<String>,
    prompt: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    model_policy: Option<String>,
    reasoning: Option<String>,
    tools: Option<RawTools>,
}

enum RawTools {
    All,
    Names(Vec<String>),
}

pub(crate) fn parse_definition(
    path: &Path,
    fallback_id: &str,
    contents: &str,
) -> Result<AgentDefinition, AgentCatalogError> {
    let (frontmatter, body) = split_frontmatter(path, contents)?;
    let raw = parse_fields(path, &frontmatter)?;
    let id_value = raw.id.as_deref().unwrap_or(fallback_id);
    let id = AgentId::new(id_value).map_err(|error| {
        AgentCatalogError::at_field(path.to_path_buf(), "id", error.to_string())
    })?;
    let description = raw
        .description
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AgentCatalogError::at_field(path.to_path_buf(), "description", "is required")
        })?;
    if description.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "description",
            "must be at most 1024 characters",
        ));
    }
    let body = body.trim().to_string();
    let prompt = match raw.prompt.as_deref().unwrap_or("extend") {
        "extend" => PromptPolicy::Extend(body),
        "replace" if body.is_empty() => {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "prompt",
                "replace requires a non-empty Markdown body",
            ))
        }
        "replace" => PromptPolicy::Replace(body),
        value => {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "prompt",
                format!("unknown value '{value}'; expected extend or replace"),
            ))
        }
    };
    let model = parse_model_policy(path, raw.model, raw.provider, raw.model_policy)?;
    let reasoning = raw
        .reasoning
        .map(|value| {
            value.parse::<ReasoningLevel>().map_err(|error| {
                AgentCatalogError::at_field(path.to_path_buf(), "reasoning", error.to_string())
            })
        })
        .transpose()?;
    let tools = match raw.tools.unwrap_or(RawTools::All) {
        RawTools::All => ToolPolicy::All,
        RawTools::Names(names) => ToolPolicy::Allow(validate_tools(path, names)?),
    };
    Ok(AgentDefinition {
        id,
        description,
        prompt,
        model,
        tools,
        reasoning,
    })
}

fn parse_model_policy(
    path: &Path,
    model: Option<String>,
    provider: Option<String>,
    policy: Option<String>,
) -> Result<ModelPolicy, AgentCatalogError> {
    let policy = policy
        .as_deref()
        .unwrap_or(if model.is_some() { "select" } else { "inherit" });
    if policy == "inherit" {
        if model.is_some() || provider.is_some() {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "model-policy",
                "inherit cannot specify model or provider",
            ));
        }
        return Ok(ModelPolicy::Inherit);
    }
    if !matches!(policy, "prefer" | "require" | "select") {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "model-policy",
            format!("unknown value '{policy}'; expected inherit, prefer, require, or select"),
        ));
    }
    let model = model
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AgentCatalogError::at_field(
                path.to_path_buf(),
                "model",
                format!("is required by model-policy '{policy}'"),
            )
        })?;
    if model.chars().any(char::is_whitespace) {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "model",
            "must not contain whitespace",
        ));
    }
    if provider
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.chars().any(char::is_whitespace))
    {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "provider",
            "must be non-empty and contain no whitespace",
        ));
    }
    let selection = ModelSelection { provider, model };
    Ok(match policy {
        "prefer" => ModelPolicy::Prefer(selection),
        "require" => ModelPolicy::Require(selection),
        "select" => ModelPolicy::Select(selection),
        _ => unreachable!(),
    })
}

fn validate_tools(path: &Path, names: Vec<String>) -> Result<Vec<String>, AgentCatalogError> {
    let mut unique = BTreeSet::new();
    for name in names {
        if !KNOWN_TOOLS.contains(&name.as_str()) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!(
                    "unknown tool '{name}'; known tools: {}",
                    KNOWN_TOOLS.join(", ")
                ),
            ));
        }
        if !unique.insert(name.clone()) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!("duplicate tool '{name}'"),
            ));
        }
    }
    Ok(unique.into_iter().collect())
}

fn split_frontmatter<'a>(
    path: &Path,
    contents: &'a str,
) -> Result<(Vec<&'a str>, String), AgentCatalogError> {
    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        return Err(AgentCatalogError::at_path(
            path.to_path_buf(),
            "must start with '---' Markdown frontmatter",
        ));
    }
    let mut frontmatter = Vec::new();
    for line in lines.by_ref() {
        if line == "---" {
            return Ok((frontmatter, lines.collect::<Vec<_>>().join("\n")));
        }
        frontmatter.push(line);
    }
    Err(AgentCatalogError::at_path(
        path.to_path_buf(),
        "unterminated frontmatter",
    ))
}

fn parse_fields(path: &Path, lines: &[&str]) -> Result<RawDefinition, AgentCatalogError> {
    let mut raw = RawDefinition::default();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        index += 1;
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(char::is_whitespace) || line.trim_start().starts_with("- ") {
            return Err(AgentCatalogError::at_path(
                path.to_path_buf(),
                format!("invalid frontmatter syntax on line {}", index + 1),
            ));
        }
        let (key, value) = line.split_once(':').ok_or_else(|| {
            AgentCatalogError::at_path(
                path.to_path_buf(),
                format!("expected 'field: value' on line {}", index + 1),
            )
        })?;
        let key = key.trim();
        if !matches!(
            key,
            "id" | "description"
                | "prompt"
                | "model"
                | "provider"
                | "model-policy"
                | "reasoning"
                | "tools"
        ) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                key,
                "unknown field",
            ));
        }
        if !seen.insert(key) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                key,
                "duplicate field",
            ));
        }
        if key == "tools" && value.trim().is_empty() {
            let mut tools = Vec::new();
            while index < lines.len() {
                let item = lines[index];
                let Some(item) = item.strip_prefix("  - ") else {
                    break;
                };
                tools.push(parse_scalar(path, "tools", item)?);
                index += 1;
            }
            raw.tools = Some(RawTools::Names(tools));
            continue;
        }
        let value = parse_scalar(path, key, value.trim())?;
        match key {
            "id" => raw.id = Some(value),
            "description" => raw.description = Some(value),
            "prompt" => raw.prompt = Some(value),
            "model" => raw.model = Some(value),
            "provider" => raw.provider = Some(value),
            "model-policy" => raw.model_policy = Some(value),
            "reasoning" => raw.reasoning = Some(value),
            "tools" if value == "all" => raw.tools = Some(RawTools::All),
            "tools" => raw.tools = Some(RawTools::Names(parse_inline_list(path, &value)?)),
            _ => unreachable!(),
        }
    }
    Ok(raw)
}

fn parse_scalar(path: &Path, field: &str, value: &str) -> Result<String, AgentCatalogError> {
    if value.is_empty() {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            field,
            "must not be empty",
        ));
    }
    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    if value.starts_with(['"', '\'']) && !quoted {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            field,
            "unterminated quoted value",
        ));
    }
    Ok(if quoted {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    })
}

fn parse_inline_list(path: &Path, value: &str) -> Result<Vec<String>, AgentCatalogError> {
    if !value.starts_with('[') || !value.ends_with(']') {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "tools",
            "expected 'all', an inline list, or an indented sequence",
        ));
    }
    let inner = &value[1..value.len() - 1];
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|item| parse_scalar(path, "tools", item.trim()))
        .collect()
}
