//! Subagent presets and the run-status contract shared between a parent rho
//! process and the `rho run --preset` children it spawns.
//!
//! Presets are markdown files with YAML frontmatter. The body becomes extra
//! system-prompt instructions for the subagent. Discovery mirrors skill
//! discovery roots (`~/.rho/agents`, `~/.agents/agents`, and per-project
//! `.agents/agents`), with built-in presets appended last so user files with
//! the same name override them.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use crate::reasoning::ReasoningLevel;

pub const RESULT_FILE_NAME: &str = "result.json";
pub const LOG_FILE_NAME: &str = "log.txt";
pub const CANCEL_FILE_NAME: &str = "cancel.requested";

const BUILTIN_PRESETS: &[(&str, &str)] = &[
    ("explorer", include_str!("builtin_agents/explorer.md")),
    ("worker", include_str!("builtin_agents/worker.md")),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    #[default]
    Keep,
    Close,
    CloseOnSuccess,
}

impl FromStr for OnExit {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "keep" => Ok(Self::Keep),
            "close" => Ok(Self::Close),
            "close-on-success" => Ok(Self::CloseOnSuccess),
            other => anyhow::bail!(
                "invalid on_exit '{other}': expected keep, close, or close-on-success"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetSource {
    BuiltIn,
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    /// Tool names the subagent may use; `None` grants the full tool set.
    pub tools: Option<Vec<String>>,
    pub on_exit: OnExit,
    /// Markdown body, appended to the subagent's system prompt.
    pub prompt: String,
    pub source: PresetSource,
}

pub fn discover(cwd: &Path) -> Vec<Preset> {
    let home = crate::paths::home_dir();
    discover_with_home(cwd, home.as_deref())
}

pub fn discover_with_home(cwd: &Path, home: Option<&Path>) -> Vec<Preset> {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".rho").join("agents"));
        roots.push(home.join(".agents").join("agents"));
    }
    roots.extend(
        crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .rev()
            .map(|path| path.join(".agents").join("agents")),
    );

    let mut discovered: Vec<Preset> = roots
        .into_iter()
        .flat_map(|root| preset_paths(&root))
        .filter_map(|path| read_preset(&path).ok())
        .collect();
    discovered.extend(BUILTIN_PRESETS.iter().map(|(name, contents)| {
        parse_preset(name, contents, PresetSource::BuiltIn).expect("embedded presets must be valid")
    }));

    let mut seen = HashSet::new();
    discovered
        .into_iter()
        .filter(|preset| seen.insert(preset.name.clone()))
        .collect()
}

pub fn find(cwd: &Path, name: &str) -> anyhow::Result<Preset> {
    discover(cwd)
        .into_iter()
        .find(|preset| preset.name == name)
        .ok_or_else(|| anyhow::anyhow!("unknown subagent preset '{name}'"))
}

fn preset_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    paths.sort();
    paths
}

fn read_preset(path: &Path) -> anyhow::Result<Preset> {
    let contents = std::fs::read_to_string(path)?;
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| anyhow::anyhow!("preset file has no valid name"))?;
    parse_preset(name, &contents, PresetSource::File(path.to_path_buf()))
}

fn parse_preset(name: &str, contents: &str, source: PresetSource) -> anyhow::Result<Preset> {
    validate_name(name)?;
    let (fields, body) = parse_frontmatter(contents)?;
    let field = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.clone())
    };
    let description = field("description")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("preset '{name}' is missing a description"))?;
    if description.len() > 1024 {
        anyhow::bail!("preset description must be at most 1024 characters");
    }
    let reasoning = field("reasoning")
        .map(|value| {
            value
                .parse::<ReasoningLevel>()
                .map_err(|error| anyhow::anyhow!("preset '{name}': {error}"))
        })
        .transpose()?;
    let on_exit = field("on_exit")
        .map(|value| value.parse::<OnExit>())
        .transpose()?
        .unwrap_or_default();
    let tools = field("tools").map(|value| parse_tool_list(&value));

    Ok(Preset {
        name: name.to_string(),
        description,
        model: field("model").filter(|value| !value.is_empty()),
        provider: field("provider").filter(|value| !value.is_empty()),
        reasoning,
        tools,
        on_exit,
        prompt: body.trim().to_string(),
        source,
    })
}

fn parse_tool_list(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| item.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Parses `---`-delimited frontmatter into key/value pairs plus the body.
fn parse_frontmatter(contents: &str) -> anyhow::Result<(Vec<(String, String)>, String)> {
    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        anyhow::bail!("preset file must start with YAML frontmatter");
    }
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut sequence_field = None;
    for line in lines.by_ref() {
        if line == "---" {
            let body: String = lines.collect::<Vec<_>>().join("\n");
            return Ok((fields, body));
        }
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let (Some(key), Some(item)) =
                (sequence_field.as_deref(), line.trim().strip_prefix("- "))
            {
                if let Some((_, value)) = fields.iter_mut().find(|(field, _)| field == key) {
                    if !value.is_empty() {
                        value.push(',');
                    }
                    value.push_str(&unquote(item.trim()));
                }
            }
            continue;
        }
        sequence_field = None;
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = unquote(value.trim());
            if value.is_empty() {
                sequence_field = Some(key.clone());
            }
            fields.push((key, value));
        }
    }
    anyhow::bail!("unterminated YAML frontmatter")
}

fn unquote(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("preset name must be 1-64 characters");
    }
    let bytes = name.as_bytes();
    if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') || name.contains("--") {
        anyhow::bail!("preset name must use single hyphen separators");
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        anyhow::bail!("preset name must be lowercase alphanumeric with hyphen separators");
    }
    Ok(())
}

/// State machine for a subagent run, persisted in the result file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    #[default]
    Starting,
    Running,
    Ok,
    Error,
    Stopped,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ok | Self::Error | Self::Stopped)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Stopped => "stopped",
        }
    }
}

/// Contents of the `--output-file` a subagent writes atomically as it runs.
///
/// The parent process reads this file for status checks and completion
/// detection; the pane or log output is display-only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunStatus {
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Writes the status file atomically (temp file + rename) so readers never
/// observe a torn write.
pub fn write_status(path: &Path, status: &RunStatus) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_string_pretty(status)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, contents)?;
    std::fs::rename(&temp, path)
}

pub fn read_status(path: &Path) -> Option<RunStatus> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Returns the cancellation marker associated with a result file.
pub fn cancel_file_for(output_file: &Path) -> PathBuf {
    output_file.with_file_name(CANCEL_FILE_NAME)
}

/// Requests graceful cancellation of the run writing `output_file`.
pub fn request_cancel(output_file: &Path) -> std::io::Result<()> {
    let cancel_file = cancel_file_for(output_file);
    if let Some(parent) = cancel_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(cancel_file, [])
}

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod tests;
