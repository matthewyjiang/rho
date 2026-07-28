//! Serializes an [`AgentDefinition`] back to the Markdown file format that
//! [`super::parser`] parses.
//!
//! The output is canonical and round-trips through the parser: serializing a
//! parsed definition and parsing the result yields an equal definition, and
//! parsing a serialized definition reproduces the serialized form (modulo the
//! canonical key order chosen here).
//!
//! Field order is fixed so diffs stay small when one field changes:
//! `id`, `description`, `prompt`, `runtime`, `model-policy`, `model`,
//! `provider`, `reasoning`, `inherit_claude_config`, `tools`.

use std::fmt::Write;

use super::definition::{
    AgentDefinition, AgentRuntimeSpec, ClaudeToolPolicy, ModelPolicy, PromptPolicy, ToolPolicy,
};

/// Serializes `definition` into the Markdown frontmatter + body file format.
pub fn serialize_definition(definition: &AgentDefinition) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    write_frontmatter(&mut out, definition);
    out.push_str("---\n");
    let body = match &definition.prompt {
        PromptPolicy::Extend(body) | PromptPolicy::Replace(body) => body.as_str(),
    };
    out.push_str(body);
    // A non-empty body should end with a single trailing newline so the file
    // reads cleanly and re-parses identically. An empty body (extend with no
    // text) stays empty.
    if !body.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn write_frontmatter(out: &mut String, definition: &AgentDefinition) {
    let _ = writeln!(out, "id: {}", definition.id.as_str());
    let _ = writeln!(out, "description: {}", scalar(&definition.description));
    match &definition.prompt {
        PromptPolicy::Extend(_) => {
            let _ = writeln!(out, "prompt: extend");
        }
        PromptPolicy::Replace(_) => {
            let _ = writeln!(out, "prompt: replace");
        }
    }
    write_runtime(out, &definition.runtime);
    write_model(out, &definition.runtime);
    write_reasoning(out, &definition.runtime);
    write_inherit_claude_config(out, &definition.runtime);
    write_tools(out, &definition.runtime);
}

fn write_runtime(out: &mut String, runtime: &AgentRuntimeSpec) {
    let _ = writeln!(out, "runtime: {}", runtime.runtime().as_str());
}

fn write_model(out: &mut String, runtime: &AgentRuntimeSpec) {
    match runtime {
        AgentRuntimeSpec::Rho { model, .. } => match model {
            ModelPolicy::Inherit => {
                let _ = writeln!(out, "model-policy: inherit");
            }
            ModelPolicy::Prefer(selection)
            | ModelPolicy::Require(selection)
            | ModelPolicy::Select(selection) => {
                let policy = match model {
                    ModelPolicy::Prefer(_) => "prefer",
                    ModelPolicy::Require(_) => "require",
                    ModelPolicy::Select(_) => "select",
                    ModelPolicy::Inherit => unreachable!(),
                };
                let _ = writeln!(out, "model-policy: {policy}");
                let _ = writeln!(out, "model: {}", scalar(&selection.model));
                if let Some(provider) = selection.provider.as_deref() {
                    let _ = writeln!(out, "provider: {}", scalar(provider));
                }
            }
        },
        AgentRuntimeSpec::ClaudeCli(config) => {
            // Claude CLI resolves model to a pass-through --model string.
            // None maps to inherit; Some maps to select. The parser accepts
            // `model: <name>` (implicit select) or `model-policy: select` +
            // `model: <name>`. Emit just the model for brevity.
            if let Some(model) = config.model.as_deref() {
                let _ = writeln!(out, "model: {}", scalar(model));
            }
        }
    }
}

fn write_reasoning(out: &mut String, runtime: &AgentRuntimeSpec) {
    let reasoning = match runtime {
        AgentRuntimeSpec::Rho { reasoning, .. } => *reasoning,
        AgentRuntimeSpec::ClaudeCli(config) => config.reasoning,
    };
    if let Some(level) = reasoning {
        let _ = writeln!(out, "reasoning: {level}");
    }
}

fn write_inherit_claude_config(out: &mut String, runtime: &AgentRuntimeSpec) {
    if let AgentRuntimeSpec::ClaudeCli(config) = runtime {
        let _ = writeln!(
            out,
            "inherit_claude_config: {}",
            config.inherit_claude_config
        );
    }
}

fn write_tools(out: &mut String, runtime: &AgentRuntimeSpec) {
    match runtime {
        AgentRuntimeSpec::Rho { tools, .. } => match tools {
            ToolPolicy::All => {
                let _ = writeln!(out, "tools: all");
            }
            ToolPolicy::Allow(capabilities) => {
                write_tool_list(
                    out,
                    capabilities.iter().map(|capability| capability.as_str()),
                );
            }
        },
        AgentRuntimeSpec::ClaudeCli(config) => match &config.tools {
            ClaudeToolPolicy::None => {
                let _ = writeln!(out, "tools: []");
            }
            ClaudeToolPolicy::Allow(tools) => {
                write_tool_list(out, tools.iter().map(String::as_str));
            }
        },
    }
}

fn write_tool_list<'a>(out: &mut String, items: impl Iterator<Item = &'a str>) {
    let collected: Vec<&str> = items.collect();
    if collected.is_empty() {
        let _ = writeln!(out, "tools: []");
        return;
    }
    let rendered = collected
        .iter()
        .map(|item| list_item(item))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(out, "tools: [{rendered}]");
}

/// Renders a scalar frontmatter value, quoting it only when the parser would
/// misread a bare value: empty values are invalid and never produced here;
/// values containing a quote char or comma are wrapped so they survive
/// [`super::parser`] intact.
fn scalar(value: &str) -> String {
    if value.is_empty() {
        // The parser rejects empty scalars; callers must prevent this. Emit a
        // quoted empty string so serialization never produces invalid output.
        return r#""""#.to_string();
    }
    let needs_quotes = value.contains('"')
        || value.contains('\'')
        || value.contains(',')
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace);
    if !needs_quotes {
        return value.to_string();
    }
    quote_value(value)
}

/// Renders one item inside a `tools: [...]` list. Items are quoted when they
/// contain a space, comma, or quote, matching the documented Claude examples
/// such as `tools: [Read, Edit, "Bash(git *)"]`.
fn list_item(value: &str) -> String {
    if value.is_empty() {
        return r#""""#.to_string();
    }
    let needs_quotes =
        value.contains(' ') || value.contains(',') || value.contains('"') || value.contains('\'');
    if !needs_quotes {
        return value.to_string();
    }
    quote_value(value)
}

/// Wraps `value` in the quote character it does not contain. If it contains
/// both, default to double quotes; the parser strips the outer pair and the
/// inner quote survives, so this is safe for the rare values that hold both.
fn quote_value(value: &str) -> String {
    if !value.contains('"') {
        format!("\"{value}\"")
    } else if !value.contains('\'') {
        format!("'{value}'")
    } else {
        format!("\"{value}\"")
    }
}

#[cfg(test)]
#[path = "serializer_tests.rs"]
mod tests;
