//! `hooks.toml` parsing and validation.
//!
//! Executable policy is deliberately kept out of `config.toml`: preferences and
//! "programs Rho will run" are different trust questions and belong in different
//! files. Unknown keys are rejected so a typo cannot silently disable a hook.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

use rho_sdk::hooks::HookEventKind;

use super::{
    matcher::{ToolMatcher, ToolMatcherError},
    HookSource,
};

/// The only schema version this build understands.
pub(crate) const HOOKS_SCHEMA_VERSION: u32 = 1;

/// Longest a single hook program may run before it is killed.
pub(crate) const MAX_HOOK_TIMEOUT: Duration = Duration::from_secs(600);

/// Raw file shape. Serde rejects unknown fields so a misspelled key fails loudly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHooksFile {
    version: u32,
    #[serde(default)]
    hook: Vec<RawHook>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHook {
    id: String,
    on: String,
    #[serde(default)]
    tools: Option<Vec<String>>,
    command: Vec<String>,
    timeout: String,
    #[serde(default)]
    env: Vec<String>,
}

/// One validated hook, resolved against the file that declared it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookDefinition {
    pub(crate) source: HookSource,
    pub(crate) id: String,
    pub(crate) event: HookEventKind,
    pub(crate) tools: ToolMatcher,
    /// Argv with the program resolved to an absolute path where determinable.
    pub(crate) command: Vec<String>,
    pub(crate) timeout: Duration,
    pub(crate) env: Vec<String>,
    pub(crate) working_directory: PathBuf,
}

impl HookDefinition {
    /// Namespaced identity used in diagnostics, denial reasons, and envelopes.
    ///
    /// Namespacing by source is what lets a project ship a hook whose id already
    /// exists in the user's file without either one breaking.
    pub fn qualified_id(&self) -> String {
        format!("{}:{}", self.source.label(), self.id)
    }

    pub fn event(&self) -> HookEventKind {
        self.event
    }

    pub fn command(&self) -> &[String] {
        &self.command
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn env(&self) -> &[String] {
        &self.env
    }

    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    pub fn tools(&self) -> &ToolMatcher {
        &self.tools
    }
}

/// A `hooks.toml` that could not be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {location}{message}")]
pub struct HookConfigError {
    pub path: PathBuf,
    pub hook_id: Option<String>,
    pub field: Option<String>,
    pub message: String,
    location: String,
}

impl HookConfigError {
    fn new(
        path: &Path,
        hook_id: Option<&str>,
        field: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        let location = match (hook_id, field) {
            (Some(id), Some(field)) => format!("hook '{id}': field '{field}': "),
            (Some(id), None) => format!("hook '{id}': "),
            (None, Some(field)) => format!("field '{field}': "),
            (None, None) => String::new(),
        };
        Self {
            path: path.to_path_buf(),
            hook_id: hook_id.map(str::to_owned),
            field: field.map(str::to_owned),
            message: message.into(),
            location,
        }
    }

    pub(crate) fn at_file(path: &Path, message: impl Into<String>) -> Self {
        Self::new(path, None, None, message)
    }

    fn at_field(path: &Path, field: &str, message: impl Into<String>) -> Self {
        Self::new(path, None, Some(field), message)
    }

    fn at_hook(path: &Path, id: &str, field: &str, message: impl Into<String>) -> Self {
        Self::new(path, Some(id), Some(field), message)
    }
}

/// Parses and validates one hooks file.
///
/// `project_root` is `Some` for a project file. Project commands must be given
/// in path form so granting trust to a workspace cannot silently bind whatever
/// `PATH` happens to resolve today.
pub fn parse_hooks_file(
    path: &Path,
    contents: &str,
    source: HookSource,
    project_root: Option<&Path>,
    canonical_tools: &[&str],
) -> Result<Vec<HookDefinition>, HookConfigError> {
    let raw: RawHooksFile = toml::from_str(contents)
        .map_err(|error| HookConfigError::at_file(path, error.message().to_owned()))?;
    if raw.version != HOOKS_SCHEMA_VERSION {
        return Err(HookConfigError::at_field(
            path,
            "version",
            format!(
                "unsupported hooks schema version {}; this build understands {HOOKS_SCHEMA_VERSION}",
                raw.version
            ),
        ));
    }

    let mut definitions: Vec<HookDefinition> = Vec::with_capacity(raw.hook.len());
    for hook in raw.hook {
        if definitions.iter().any(|existing| existing.id == hook.id) {
            return Err(HookConfigError::at_hook(
                path,
                &hook.id,
                "id",
                "duplicate hook ID in this file",
            ));
        }
        definitions.push(validate(path, hook, source, project_root, canonical_tools)?);
    }
    Ok(definitions)
}

fn validate(
    path: &Path,
    raw: RawHook,
    source: HookSource,
    project_root: Option<&Path>,
    canonical_tools: &[&str],
) -> Result<HookDefinition, HookConfigError> {
    let id = raw.id.trim().to_owned();
    if id.is_empty() {
        return Err(HookConfigError::at_field(path, "id", "hook ID is empty"));
    }
    if !id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(HookConfigError::at_hook(
            path,
            &id,
            "id",
            "hook ID may contain only ASCII letters, digits, '-', '_', and '.'",
        ));
    }

    let event = HookEventKind::from_wire_name(&raw.on).ok_or_else(|| {
        HookConfigError::at_hook(
            path,
            &id,
            "on",
            format!(
                "unknown event '{}'; known events are {}",
                raw.on,
                known_events()
            ),
        )
    })?;

    let tools = build_matcher(path, &id, event, raw.tools, canonical_tools)?;
    let command = resolve_command(path, &id, raw.command, source, project_root)?;
    let timeout = parse_timeout(path, &id, &raw.timeout)?;
    let env = validate_env(path, &id, raw.env)?;

    Ok(HookDefinition {
        source,
        id,
        event,
        tools,
        command,
        timeout,
        env,
        working_directory: project_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf()),
    })
}

fn build_matcher(
    path: &Path,
    id: &str,
    event: HookEventKind,
    tools: Option<Vec<String>>,
    canonical_tools: &[&str],
) -> Result<ToolMatcher, HookConfigError> {
    let Some(patterns) = tools else {
        return Ok(ToolMatcher::any());
    };
    if !matches!(
        event,
        HookEventKind::BeforeToolUse | HookEventKind::AfterToolUse
    ) {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "tools",
            format!("event '{event}' has no tool to match"),
        ));
    }
    ToolMatcher::new(patterns, canonical_tools).map_err(|error| match error {
        ToolMatcherError::Empty => {
            HookConfigError::at_hook(path, id, "tools", "tool list is empty")
        }
        ToolMatcherError::BlankPattern => {
            HookConfigError::at_hook(path, id, "tools", "tool pattern is blank")
        }
        ToolMatcherError::UnsupportedGlob { pattern } => HookConfigError::at_hook(
            path,
            id,
            "tools",
            format!("tool pattern '{pattern}' is unsupported; use a name or a single trailing '*'"),
        ),
        ToolMatcherError::UnknownTool { pattern } => HookConfigError::at_hook(
            path,
            id,
            "tools",
            format!(
                "tool '{pattern}' is not in the canonical tool list: {}",
                canonical_tools.join(", ")
            ),
        ),
    })
}

fn resolve_command(
    path: &Path,
    id: &str,
    command: Vec<String>,
    source: HookSource,
    project_root: Option<&Path>,
) -> Result<Vec<String>, HookConfigError> {
    let Some(program) = command.first() else {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "command",
            "command argv is empty",
        ));
    };
    if program.trim().is_empty() {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "command",
            "command program is blank",
        ));
    }

    let program_path = PathBuf::from(program);
    let is_path_form =
        program_path.is_absolute() || program.contains('/') || program.contains('\\');
    if source == HookSource::Project && !is_path_form {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "command",
            format!(
                "project hooks must name a program by path, not a bare PATH name like '{program}'; \
                 use './relative/path' or an absolute path"
            ),
        ));
    }

    let mut resolved = command;
    if let Some(root) = project_root.filter(|_| !program_path.is_absolute() && is_path_form) {
        // Resolve once at load so diagnostics and the trust prompt show the exact
        // file that will be executed, not a path relative to whatever cwd applies.
        resolved[0] = crate::paths::display(&join_relative(root, &program_path));
    }

    let executable = Path::new(&resolved[0]);
    if executable.is_absolute() && !executable.exists() {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "command",
            format!("hook program '{}' does not exist", resolved[0]),
        ));
    }
    Ok(resolved)
}

/// Joins `relative` onto `root`, dropping `./` segments so the rendered path a
/// user is asked to trust reads like the file it points at.
fn join_relative(root: &Path, relative: &Path) -> PathBuf {
    let mut joined = root.to_path_buf();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            other => joined.push(other),
        }
    }
    joined
}

fn parse_timeout(path: &Path, id: &str, raw: &str) -> Result<Duration, HookConfigError> {
    let timeout = humantime::parse_duration(raw.trim()).map_err(|error| {
        HookConfigError::at_hook(path, id, "timeout", format!("invalid duration: {error}"))
    })?;
    if timeout.is_zero() {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "timeout",
            "timeout must be greater than zero",
        ));
    }
    if timeout > MAX_HOOK_TIMEOUT {
        return Err(HookConfigError::at_hook(
            path,
            id,
            "timeout",
            format!(
                "timeout {raw} exceeds the {} second maximum",
                MAX_HOOK_TIMEOUT.as_secs()
            ),
        ));
    }
    Ok(timeout)
}

fn validate_env(
    path: &Path,
    id: &str,
    mut names: Vec<String>,
) -> Result<Vec<String>, HookConfigError> {
    for name in &names {
        if name.trim().is_empty() {
            return Err(HookConfigError::at_hook(
                path,
                id,
                "env",
                "environment variable name is blank",
            ));
        }
        if name.contains('=') {
            return Err(HookConfigError::at_hook(
                path,
                id,
                "env",
                format!("'{name}' is a name/value pair; list only variable names"),
            ));
        }
    }
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

fn known_events() -> String {
    HookEventKind::ALL
        .iter()
        .map(|event| event.wire_name())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
