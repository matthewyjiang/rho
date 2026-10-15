//! `/init`: project instruction onboarding through the built-in `rho-init`
//! skill. Hosts check availability and build the model prompt here, then
//! start the turn with a host-requested `skill` call for [`SKILL`].

use std::path::{Path, PathBuf};

use crate::permission::PermissionMode;

/// Built-in skill the init turn loads.
pub(crate) const SKILL: &str = "rho-init";

/// Why `/init` cannot start. Displays as a transcript error.
#[derive(Debug)]
pub(crate) enum InitUnavailable {
    PlanMode,
    MissingTools(Vec<&'static str>),
    Inspect {
        target: PathBuf,
        error: std::io::Error,
    },
}

impl std::fmt::Display for InitUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("could not start init: ")?;
        match self {
            Self::PlanMode => formatter.write_str("file writes are denied in plan permission mode"),
            Self::MissingTools(tools) => write!(
                formatter,
                "active agent is missing required tools: {}",
                tools.join(", ")
            ),
            Self::Inspect { target, error } => {
                write!(formatter, "could not inspect {}: {error}", target.display())
            }
        }
    }
}

impl std::error::Error for InitUnavailable {}

/// The init turn's model prompt for `cwd`, or why init cannot run with this
/// permission mode and these tools.
pub(crate) fn model_prompt(
    cwd: &Path,
    mode: PermissionMode,
    has_tool: impl Fn(&str) -> bool,
) -> Result<String, InitUnavailable> {
    if mode == PermissionMode::Plan {
        return Err(InitUnavailable::PlanMode);
    }
    let mut missing_tools = ["skill", "write"]
        .into_iter()
        .filter(|name| !has_tool(name))
        .collect::<Vec<_>>();
    if !rho_tools::EditFormat::ALL
        .iter()
        .any(|format| has_tool(format.tool_name()))
    {
        missing_tools.push("file-edit tool");
    }
    if !missing_tools.is_empty() {
        return Err(InitUnavailable::MissingTools(missing_tools));
    }

    // Use the same git-root-to-cwd boundary as AGENTS.md discovery.
    let root = crate::workspace::project_ancestor_dirs(cwd)
        .into_iter()
        .next()
        .unwrap_or_else(|| cwd.to_path_buf());
    let target = root.join("AGENTS.md");
    let exists = target
        .try_exists()
        .map_err(|error| InitUnavailable::Inspect {
            target: target.clone(),
            error,
        })?;
    Ok(format!(
        "Survey the repository and create or update its project instructions using the {SKILL} skill.\nTarget path: {}\nTarget exists: {exists}",
        crate::paths::prompt_data(&target),
    ))
}
