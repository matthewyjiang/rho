use std::{collections::BTreeSet, path::Path, str::FromStr};

use rho_providers::reasoning::ReasoningLevel;

use super::{
    catalog::AgentCatalogError,
    definition::{
        AgentDefinition, AgentId, AgentRuntime, AgentRuntimeSpec, ClaudeAgentConfig,
        ClaudeToolPolicy, ModelPolicy, ModelSelection, PromptPolicy, ToolCapability,
        ToolCapabilitySet, ToolPolicy, BUILTIN_TOOL_CAPABILITIES,
    },
};

const MAX_DESCRIPTION_LEN: usize = 1024;
const RHO_TOOLS_EXAMPLE: &str = "tools: [read_file, shell]";
const CLAUDE_TOOLS_EXAMPLE: &str = "tools: [Read, Edit, \"Bash(git *)\"]";

#[derive(Default)]
struct RawDefinition {
    id: Option<String>,
    description: Option<String>,
    prompt: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    auth: Option<String>,
    model_policy: Option<String>,
    reasoning: Option<String>,
    runtime: Option<String>,
    inherit_claude_config: Option<bool>,
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

    // Runtime is resolved before tools so tool vocabulary does not depend on
    // frontmatter key order.
    let runtime = match raw.runtime.as_deref() {
        None => AgentRuntime::Rho,
        Some(value) => AgentRuntime::from_str(value).map_err(|error| {
            AgentCatalogError::at_field(path.to_path_buf(), "runtime", error.to_string())
        })?,
    };

    let model = parse_model_policy(
        path,
        runtime,
        raw.model,
        raw.provider,
        raw.auth,
        raw.model_policy,
    )?;
    let reasoning = raw
        .reasoning
        .map(|value| {
            value.parse::<ReasoningLevel>().map_err(|error| {
                AgentCatalogError::at_field(path.to_path_buf(), "reasoning", error.to_string())
            })
        })
        .transpose()?;
    if runtime == AgentRuntime::ClaudeCli {
        if let Some(level) = reasoning {
            // Claude `--effort` accepts low/medium/high/xhigh/max only.
            if matches!(level, ReasoningLevel::Off | ReasoningLevel::Minimal) {
                return Err(AgentCatalogError::at_field(
                    path.to_path_buf(),
                    "reasoning",
                    format!(
                        "value '{level}' is not a Claude Code effort level; \
expected one of: low, medium, high, xhigh, max (omit to inherit Claude's default)"
                    ),
                ));
            }
        }
    }
    Ok(AgentDefinition {
        id,
        description,
        prompt,
        runtime: parse_runtime_spec(
            path,
            runtime,
            raw.tools,
            raw.inherit_claude_config.unwrap_or(false),
            model,
            reasoning,
        )?,
    })
}

fn parse_model_policy(
    path: &Path,
    runtime: AgentRuntime,
    model: Option<String>,
    provider: Option<String>,
    auth: Option<String>,
    policy: Option<String>,
) -> Result<ModelPolicy, AgentCatalogError> {
    if runtime == AgentRuntime::ClaudeCli {
        if provider.is_some() {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "provider",
                "is not valid with runtime: claude-cli; set model only (passed through as --model)",
            ));
        }
        if auth.is_some() {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "auth",
                "is not valid with runtime: claude-cli; set model only (passed through as --model)",
            ));
        }
        if policy
            .as_deref()
            .is_some_and(|value| value != "inherit" && value != "select")
        {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "model-policy",
                "with runtime: claude-cli expected inherit or select (or omit model-policy and set model)",
            ));
        }
        // Empty quoted models such as model: "" fail in parse_scalar already.
        // Still reject inherit + explicit model and select without model here.
        return match (policy.as_deref(), model) {
            (Some("inherit"), Some(_)) => Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "model-policy",
                "inherit cannot specify model",
            )),
            (Some("select"), None) => Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "model",
                "is required by model-policy 'select'",
            )),
            (_, None) => Ok(ModelPolicy::Inherit),
            (_, Some(model)) => {
                validate_model_name(path, &model)?;
                if model.starts_with('@') {
                    return Err(AgentCatalogError::at_field(
                        path.to_path_buf(),
                        "model",
                        format!(
                            "runtime: claude-cli does not resolve Rho model aliases; \
set a Claude model name or alias (for example opus), not '{model}'"
                        ),
                    ));
                }
                Ok(ModelPolicy::Select(ModelSelection {
                    provider: None,
                    model,
                    auth: None,
                }))
            }
        };
    }

    let policy = policy
        .as_deref()
        .unwrap_or(if model.is_some() { "select" } else { "inherit" });
    if policy == "inherit" {
        if model.is_some() || provider.is_some() || auth.is_some() {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "model-policy",
                "inherit cannot specify model, provider, or auth",
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
    validate_model_name(path, &model)?;
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
    if auth
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.chars().any(char::is_whitespace))
    {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "auth",
            "must be non-empty and contain no whitespace",
        ));
    }
    if let Some(auth_value) = auth.as_deref() {
        if rho_providers::provider::resolve_auth_mode(auth_value).is_none() {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "auth",
                format!("unknown auth profile '{auth_value}'"),
            ));
        }
        if let Some(provider_value) = provider.as_deref() {
            rho_providers::provider::resolve_profile_exact(provider_value, auth_value).map_err(
                |error| {
                    AgentCatalogError::at_field(
                        path.to_path_buf(),
                        "auth",
                        format!("is not valid for provider '{provider_value}': {error}"),
                    )
                },
            )?;
        }
    }
    let selection = ModelSelection {
        provider,
        model,
        auth,
    };
    Ok(match policy {
        "prefer" => ModelPolicy::Prefer(selection),
        "require" => ModelPolicy::Require(selection),
        "select" => ModelPolicy::Select(selection),
        _ => unreachable!(),
    })
}

fn validate_model_name(path: &Path, model: &str) -> Result<(), AgentCatalogError> {
    if model.is_empty() {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "model",
            "must not be empty",
        ));
    }
    if model.chars().any(char::is_whitespace) {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "model",
            "must not contain whitespace",
        ));
    }
    Ok(())
}

/// Collects the runtime axis into one value: the harness plus the tool
/// vocabulary and settings only that harness accepts.
fn parse_runtime_spec(
    path: &Path,
    runtime: AgentRuntime,
    tools: Option<RawTools>,
    inherit_claude_config: bool,
    model: ModelPolicy,
    reasoning: Option<ReasoningLevel>,
) -> Result<AgentRuntimeSpec, AgentCatalogError> {
    match runtime {
        AgentRuntime::Rho => {
            if inherit_claude_config {
                return Err(AgentCatalogError::at_field(
                    path.to_path_buf(),
                    "inherit_claude_config",
                    "is only valid with runtime: claude-cli",
                ));
            }
            let tools = match tools.unwrap_or(RawTools::All) {
                RawTools::All => ToolPolicy::All,
                RawTools::Names(names) => ToolPolicy::Allow(validate_rho_tools(path, names)?),
            };
            Ok(AgentRuntimeSpec::Rho {
                tools,
                model,
                reasoning,
            })
        }
        AgentRuntime::ClaudeCli => {
            let tools = match tools {
                None => ClaudeToolPolicy::None,
                Some(RawTools::All) => {
                    return Err(AgentCatalogError::at_field(
                        path.to_path_buf(),
                        "tools",
                        format!(
                            "runtime: claude-cli does not support tools: all; list Claude tool names, for example {CLAUDE_TOOLS_EXAMPLE}"
                        ),
                    ))
                }
                Some(RawTools::Names(names)) => {
                    let names = validate_claude_tools(path, names)?;
                    if names.is_empty() {
                        ClaudeToolPolicy::None
                    } else {
                        ClaudeToolPolicy::Allow(names)
                    }
                }
            };
            let model = match model {
                ModelPolicy::Inherit => None,
                ModelPolicy::Select(selection)
                | ModelPolicy::Prefer(selection)
                | ModelPolicy::Require(selection) => Some(selection.model),
            };
            Ok(AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
                tools,
                inherit_claude_config,
                model,
                reasoning,
            }))
        }
    }
}

fn validate_rho_tools(
    path: &Path,
    names: Vec<String>,
) -> Result<ToolCapabilitySet, AgentCatalogError> {
    let mut capabilities = ToolCapabilitySet::new();
    for name in names {
        if looks_like_claude_tool(&name) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!(
                    "tool '{name}' looks like a Claude Code tool name, but runtime is rho; use Rho capabilities, for example {RHO_TOOLS_EXAMPLE}"
                ),
            ));
        }
        let capability = ToolCapability::parse(name.clone());
        if matches!(capability, ToolCapability::Extension(_)) {
            let known = BUILTIN_TOOL_CAPABILITIES
                .iter()
                .map(ToolCapability::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!(
                    "unknown tool '{name}' for runtime: rho; known tools: {known}. Example: {RHO_TOOLS_EXAMPLE}"
                ),
            ));
        }
        if !capabilities.insert(capability) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!("duplicate tool '{name}'"),
            ));
        }
    }
    Ok(capabilities)
}

fn validate_claude_tools(
    path: &Path,
    names: Vec<String>,
) -> Result<Vec<String>, AgentCatalogError> {
    let mut tools = Vec::with_capacity(names.len());
    let mut seen = BTreeSet::new();
    for name in names {
        if looks_like_rho_tool(&name) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!(
                    "tool '{name}' is a Rho capability, but runtime is claude-cli; use Claude Code tool names, for example {CLAUDE_TOOLS_EXAMPLE}"
                ),
            ));
        }
        validate_claude_tool_shape(path, &name)?;
        if !seen.insert(name.clone()) {
            return Err(AgentCatalogError::at_field(
                path.to_path_buf(),
                "tools",
                format!("duplicate tool '{name}'"),
            ));
        }
        tools.push(name);
    }
    Ok(tools)
}

fn looks_like_claude_tool(name: &str) -> bool {
    name.contains('(')
        || name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn looks_like_rho_tool(name: &str) -> bool {
    // Exact Rho capability names only. Claude/MCP names may contain underscores.
    !matches!(
        ToolCapability::parse(name.to_string()),
        ToolCapability::Extension(_)
    )
}

fn validate_claude_tool_shape(path: &Path, name: &str) -> Result<(), AgentCatalogError> {
    let invalid = |reason: String| {
        AgentCatalogError::at_field(
            path.to_path_buf(),
            "tools",
            format!(
                "{reason}; Claude tools use names like Read, Edit, or Bash(git *). Example: {CLAUDE_TOOLS_EXAMPLE}"
            ),
        )
    };
    // Outer shape only: Tool or Tool(specifier). The interior of a specifier is
    // opaque non-control text except for CLI list delimiters that cannot
    // round-trip through `--allowedTools` unchanged. Reject control characters
    // anywhere and malformed outer names.
    if name.is_empty() {
        return Err(invalid("invalid Claude tool name ''".into()));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid(format!("invalid Claude tool name '{name}'")));
    }
    if let Some(open_idx) = name.find('(') {
        if !name.ends_with(')') {
            return Err(invalid(format!(
                "invalid Claude tool name '{name}': specifier must end the name"
            )));
        }
        if open_idx == 0 {
            return Err(invalid(format!(
                "invalid Claude tool name '{name}': missing tool name before specifier"
            )));
        }
        // Only the first '(' opens the outer specifier; the matching final ')'
        // closes it. Interior bytes may contain nested parentheses.
        let tool = &name[..open_idx];
        let specifier = &name[open_idx + 1..name.len() - 1];
        validate_claude_base_name(path, tool)?;
        if specifier.chars().any(char::is_control) {
            return Err(invalid(format!(
                "invalid Claude tool specifier in '{name}'"
            )));
        }
        // Claude's CLI splits --allowedTools on commas and spaces. A pattern
        // containing those delimiters cannot round-trip unchanged, so reject it
        // at parse time with a clear error rather than silently reshaping it.
        if specifier.contains(',') {
            return Err(invalid(format!(
                "invalid Claude tool pattern '{name}': commas cannot round-trip through Claude --allowedTools"
            )));
        }
    } else if name.contains(')') {
        return Err(invalid(format!(
            "invalid Claude tool name '{name}': stray closing parenthesis"
        )));
    } else {
        // Base-name charset already rejects whitespace.
        validate_claude_base_name(path, name)?;
    }
    Ok(())
}

fn validate_claude_base_name(path: &Path, name: &str) -> Result<(), AgentCatalogError> {
    // Shape only. Plugins and MCP may introduce names, so parse does not use a
    // fixed catalog. Auto / Allow edits fail closed at spawn unless the name is
    // a proven no-prompt Claude built-in for that Rho approval class.
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "tools",
            format!(
                "invalid Claude tool name '{name}'; Claude tools use names like Read, Edit, or Bash(git *). Example: {CLAUDE_TOOLS_EXAMPLE}"
            ),
        ));
    }
    Ok(())
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
                | "auth"
                | "model-policy"
                | "reasoning"
                | "runtime"
                | "inherit_claude_config"
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
            "auth" => raw.auth = Some(value),
            "model-policy" => raw.model_policy = Some(value),
            "reasoning" => raw.reasoning = Some(value),
            "runtime" => raw.runtime = Some(value),
            "inherit_claude_config" => {
                raw.inherit_claude_config = Some(parse_bool(path, "inherit_claude_config", &value)?)
            }
            "tools" if value == "all" => raw.tools = Some(RawTools::All),
            "tools" => raw.tools = Some(RawTools::Names(parse_inline_list(path, &value)?)),
            _ => unreachable!(),
        }
    }
    Ok(raw)
}

fn parse_bool(path: &Path, field: &str, value: &str) -> Result<bool, AgentCatalogError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            field,
            format!("unknown value '{value}'; expected true or false"),
        )),
    }
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

/// Parses a bracket tools list like `[read_file, shell]` or `[Read, "Bash(git *)"]`.
/// Used by the in-TUI agent editor so draft tool text shares the parser's rules.
pub(crate) fn parse_tools_list_text(value: &str) -> Result<Vec<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let path = Path::new("<draft>");
    parse_inline_list(path, value).map_err(|error| error.message)
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
    // Support quoted items so Claude patterns like "Bash(git *)" round-trip.
    parse_comma_separated_scalars(path, inner)
}

fn parse_comma_separated_scalars(
    path: &Path,
    inner: &str,
) -> Result<Vec<String>, AgentCatalogError> {
    let mut items = Vec::new();
    let mut current = String::new();
    let chars = inner.chars();
    let mut in_single = false;
    let mut in_double = false;
    for ch in chars {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                // Keep quotes so parse_scalar owns quote stripping and empty
                // quoted values such as "" still fail validation.
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            ',' if !in_single && !in_double => {
                items.push(parse_scalar(path, "tools", current.trim())?);
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if in_single || in_double {
        return Err(AgentCatalogError::at_field(
            path.to_path_buf(),
            "tools",
            "unterminated quoted value",
        ));
    }
    if !current.trim().is_empty() || !items.is_empty() {
        // Trailing comma yields an empty final item, which parse_scalar rejects.
        items.push(parse_scalar(path, "tools", current.trim())?);
    }
    Ok(items)
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
