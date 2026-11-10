//! Workspace workflow entry files and the `/workflow:` commands that start
//! their runs. The TUI hub lists these sources; a session's prompts resolve
//! `/workflow:<name> [inputs]`, `/workflow:plan/<plan id>`, and
//! `/workflow:run/<run id> [recover]` through [`resolve_launch`].

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::workflow::{PlanId, RunId};

/// Slash-command prefix for starting one discovered workflow.
pub(crate) const COMMAND_PREFIX: &str = "workflow:";
/// What follows the prefix to name a saved plan, and a run.
const PLAN_PREFIX: &str = "plan/";
const RUN_PREFIX: &str = "run/";
/// The argument that lets a resumed run repeat nodes of unknown outcome.
const RECOVER: &str = "recover";

/// A workflow source the hub can validate or plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiscoveredWorkflow {
    /// Workspace-relative path using `/` separators.
    pub(crate) relative_path: String,
    /// Absolute path to the entry `.star` file.
    pub(crate) absolute_path: PathBuf,
    /// Short label for picker rows and the `/workflow:<label>` command.
    pub(crate) label: String,
}

impl DiscoveredWorkflow {
    /// The `workflow:<label>` command name, or `None` when the label cannot
    /// be a single slash-command token.
    pub(crate) fn command_name(&self) -> Option<String> {
        (!self.label.is_empty() && !self.label.contains(char::is_whitespace))
            .then(|| format!("{COMMAND_PREFIX}{}", self.label))
    }

    /// `/workflow:<label> [inputs]`, for command menus.
    pub(crate) fn command_usage(&self) -> Option<String> {
        self.command_name()
            .map(|name| format!("/{name} [inputs JSON]"))
    }

    /// One-line command description naming the entry file.
    pub(crate) fn command_description(&self) -> String {
        format!("start workflow {}", self.relative_path)
    }
}

/// A `/workflow:<name> [inputs]` request resolved against the workspace.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StartCommand {
    pub(crate) workflow: DiscoveredWorkflow,
    /// Explicit input values; omitted inputs use their declared defaults.
    pub(crate) inputs: BTreeMap<String, serde_json::Value>,
}

/// What a `/workflow:` prompt starts in the background.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WorkflowLaunch {
    /// A new run of a workspace workflow, planned now.
    Source(StartCommand),
    /// A new run of a saved plan.
    Plan(PlanId),
    /// A run that stopped, taken up again.
    Resume {
        run: RunId,
        /// Whether nodes whose outcome is unknown run again: the person
        /// said nothing of theirs still runs.
        recover_uncertain: bool,
    },
}

/// Finds workflow entry files under `.rho/workflows`.
///
/// Accepts:
/// - `.rho/workflows/<name>.star`
/// - `.rho/workflows/<name>/workflow.star`
///
/// Other `.star` files inside a workflow folder, such as `common.star`, are
/// always treated as helpers and skipped. Symlink entries are also skipped to
/// match source loading, which rejects symlinks so frozen source digests cannot
/// escape the workspace.
pub(crate) fn discover_workflow_sources(workspace: &Path) -> Vec<DiscoveredWorkflow> {
    let root = workspace.join(".rho").join("workflows");
    if !root.is_dir() {
        return Vec::new();
    }

    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() {
            if is_star_file(&path) {
                push_discovered(workspace, path, &mut found);
            }
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        let entry_star = path.join("workflow.star");
        if std::fs::symlink_metadata(&entry_star).is_ok_and(|metadata| metadata.is_file()) {
            push_discovered(workspace, entry_star, &mut found);
        }
    }

    found.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    found
}

/// Workflows that have a command, one per command name. When `<name>.star`
/// and `<name>/workflow.star` both exist, or two names differ only by case,
/// the name is listed once and [`resolve_start_command`] reports the ambiguity.
pub(crate) fn command_workflows(workspace: &Path) -> Vec<DiscoveredWorkflow> {
    let mut workflows = discover_workflow_sources(workspace);
    workflows.retain(|workflow| workflow.command_name().is_some());
    workflows.sort_by_cached_key(|workflow| workflow.label.to_ascii_lowercase());
    workflows.dedup_by(|left, right| left.label.eq_ignore_ascii_case(&right.label));
    workflows
}

/// Resolves the slash token `name` (without `/`) and its trailing `args`.
///
/// The prefix and the name match without regard to ASCII case, like
/// `/prompt:<name>`. `None` means `name` is not a workflow command.
/// `Some(Err)` carries a user-facing reason: no such workflow, an ambiguous
/// name, or inputs that are not one JSON object.
pub(crate) fn resolve_start_command(
    workspace: &Path,
    name: &str,
    args: &str,
) -> Option<Result<StartCommand, String>> {
    name.get(..COMMAND_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(COMMAND_PREFIX))
        .map(|_| resolve_label(workspace, &name[COMMAND_PREFIX.len()..], args))
}

/// Resolves the slash token `name` (without `/`) and its trailing `args` to
/// what it starts: a workspace workflow by name, as
/// [`resolve_start_command`] does, or `plan/<plan id>` and
/// `run/<run id> [recover]`, the hub's rows for saved plans and stopped
/// runs. A name is a file stem, so it never contains the `/` those have.
pub(crate) fn resolve_launch(
    workspace: &Path,
    name: &str,
    args: &str,
) -> Option<Result<WorkflowLaunch, String>> {
    let target = name
        .get(..COMMAND_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(COMMAND_PREFIX))
        .map(|_| &name[COMMAND_PREFIX.len()..])?;
    let args = args.trim();
    Some(if let Some(plan) = target.strip_prefix(PLAN_PREFIX) {
        match (PlanId::from_str(plan), args) {
            (Ok(plan), "") => Ok(WorkflowLaunch::Plan(plan)),
            (Ok(_), _) => Err("a saved plan takes no inputs".to_owned()),
            (Err(_), _) => Err(format!("'{plan}' is not a plan id")),
        }
    } else if let Some(run) = target.strip_prefix(RUN_PREFIX) {
        let recover_uncertain = match args {
            "" => Ok(false),
            RECOVER => Ok(true),
            other => Err(format!("unknown option '{other}'; use `{RECOVER}`")),
        };
        match (RunId::from_str(run), recover_uncertain) {
            (Ok(run), Ok(recover_uncertain)) => Ok(WorkflowLaunch::Resume {
                run,
                recover_uncertain,
            }),
            (Ok(_), Err(detail)) => Err(detail),
            (Err(_), _) => Err(format!("'{run}' is not a run id")),
        }
    } else {
        resolve_label(workspace, target, args).map(WorkflowLaunch::Source)
    })
}

impl WorkflowLaunch {
    /// The prompt that starts a run of saved plan `plan`.
    pub(crate) fn plan_command(plan: PlanId) -> String {
        format!("/{COMMAND_PREFIX}{PLAN_PREFIX}{plan}")
    }

    /// The prompt that takes run `run` up again.
    pub(crate) fn resume_command(run: RunId, recover_uncertain: bool) -> String {
        let command = format!("/{COMMAND_PREFIX}{RUN_PREFIX}{run}");
        if recover_uncertain {
            format!("{command} {RECOVER}")
        } else {
            command
        }
    }
}

fn resolve_label(workspace: &Path, label: &str, args: &str) -> Result<StartCommand, String> {
    let mut matches = discover_workflow_sources(workspace)
        .into_iter()
        .filter(|workflow| workflow.label.eq_ignore_ascii_case(label))
        .collect::<Vec<_>>();
    let workflow = match matches.len() {
        0 => return Err(format!("no workflow named '{label}' under .rho/workflows")),
        1 => matches.remove(0),
        _ => {
            let paths = matches
                .iter()
                .map(|workflow| workflow.relative_path.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "'{label}' matches more than one workflow ({paths}); rename one"
            ));
        }
    };
    Ok(StartCommand {
        workflow,
        inputs: parse_inputs(args)?,
    })
}

/// Empty args mean default inputs; anything else must be one JSON object.
fn parse_inputs(args: &str) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let args = args.trim();
    if args.is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(args).map_err(|error| {
        format!("inputs must be one JSON object, such as {{\"target\": \"src\"}}: {error}")
    })
}

fn is_star_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("star")
}

fn push_discovered(workspace: &Path, absolute: PathBuf, out: &mut Vec<DiscoveredWorkflow>) {
    let Ok(relative) = absolute.strip_prefix(workspace) else {
        return;
    };
    let relative_path = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if relative_path.is_empty() {
        return;
    }
    let label = workflow_label(&relative_path);
    out.push(DiscoveredWorkflow {
        relative_path,
        absolute_path: absolute,
        label,
    });
}

fn workflow_label(relative_path: &str) -> String {
    let path = Path::new(relative_path);
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "workflow.star")
    {
        if let Some(parent) = path.parent().and_then(|parent| parent.file_name()) {
            return parent.to_string_lossy().into_owned();
        }
    }
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative_path.to_owned())
}

#[cfg(test)]
#[path = "discover_tests.rs"]
mod tests;
