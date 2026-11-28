//! Discovery, trust, and ordering of hook definitions.

use std::path::{Path, PathBuf};

use crate::tools::canonical_tool_names;
use rho_sdk::hooks::HookEventKind;

use super::{
    config::{
        parse_hooks_file, ConfiguredHookEvent, HookConfigError, HookDefinition,
        WorkflowHookEventKind,
    },
    matcher::ToolMatcher,
    HookSource,
};

pub use crate::workspace::{ProjectTrust, TRUST_PROJECT_HOOKS_ENV};

/// Why a project hooks file was skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedProjectHooks {
    pub path: PathBuf,
    definitions: Vec<HookDefinition>,
    error: Option<HookConfigError>,
}

impl SkippedProjectHooks {
    pub fn error(&self) -> Option<&HookConfigError> {
        self.error.as_ref()
    }
}

/// Loaded hooks plus what discovery decided along the way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookCatalog {
    hooks: Vec<HookDefinition>,
    files: Vec<PathBuf>,
    skipped_untrusted: Option<SkippedProjectHooks>,
}

impl HookCatalog {
    /// Loads user hooks, then project hooks when the workspace is trusted.
    ///
    /// User hooks are always eligible because they are the user's own files.
    /// A malformed *trusted* file is an error: silently running with half a
    /// policy is worse than refusing to start.
    pub fn discover(
        rho_home: Option<&Path>,
        project_root: Option<&Path>,
        trust: ProjectTrust,
    ) -> Result<Self, HookConfigError> {
        let mut catalog = Self::default();
        if let Some(home) = rho_home {
            catalog.add(HooksFileRead::user(home))?;
        }
        if let Some(root) = project_root {
            catalog.add(HooksFileRead::project(root, trust))?;
        }
        Ok(catalog)
    }

    fn add(&mut self, file: HooksFileRead) -> Result<(), HookConfigError> {
        match file.outcome {
            HooksFileOutcome::Absent => {}
            HooksFileOutcome::Loaded(hooks) => {
                self.files.push(file.path);
                self.hooks.extend(hooks);
            }
            HooksFileOutcome::Untrusted { definitions, error } => {
                self.skipped_untrusted = Some(SkippedProjectHooks {
                    path: file.path,
                    definitions,
                    error,
                });
            }
            HooksFileOutcome::Invalid(error) => return Err(error),
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Drops the observational hooks, for a session only the blocking
    /// gate hears of.
    pub(crate) fn retain_blocking(&mut self) {
        self.hooks.retain(|hook| hook.event().is_blocking());
    }

    /// Whether any blocking `before_tool_use` hook is configured.
    pub fn has_blocking_hooks(&self) -> bool {
        self.hooks.iter().any(|hook| hook.event().is_blocking())
    }

    /// Whether any observational hook is configured.
    pub fn has_observational_hooks(&self) -> bool {
        self.hooks.iter().any(|hook| !hook.event().is_blocking())
    }

    #[cfg(test)]
    pub fn hooks(&self) -> &[HookDefinition] {
        &self.hooks
    }

    /// Files that were read, for reload and diagnostics.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// The project file that exists but was ignored for lack of trust.
    pub fn skipped_untrusted(&self) -> Option<&SkippedProjectHooks> {
        self.skipped_untrusted.as_ref()
    }

    /// Hooks for `event` that also match `tool`, in configured order.
    ///
    /// Order is user file first, then project file, each in file order. That is
    /// the order blocking hooks run in, and first denial wins.
    pub fn matching(&self, event: HookEventKind, tool: Option<&str>) -> Vec<&HookDefinition> {
        self.matching_configured(ConfiguredHookEvent::Sdk(event), tool)
    }

    pub(crate) fn matching_workflow(&self, event: WorkflowHookEventKind) -> Vec<&HookDefinition> {
        self.matching_configured(ConfiguredHookEvent::Workflow(event), None)
    }

    pub(crate) fn matching_configured(
        &self,
        event: ConfiguredHookEvent,
        tool: Option<&str>,
    ) -> Vec<&HookDefinition> {
        self.hooks
            .iter()
            .filter(|hook| hook.event() == event)
            .filter(|hook| match tool {
                Some(name) => hook.tools().matches(name),
                None => true,
            })
            .collect()
    }

    /// Human-readable spawn contract shown before trusted hooks run.
    ///
    /// Everything that decides what actually executes is listed: the resolved
    /// argv, working directory, timeout, and the exact environment the child
    /// receives. Trust is meaningless if the user cannot see this first.
    pub fn spawn_contract(&self) -> Vec<HookSpawnContract> {
        self.hooks
            .iter()
            .map(|hook| spawn_contract(hook, true))
            .chain(
                self.skipped_untrusted
                    .iter()
                    .flat_map(|skipped| skipped.definitions.iter())
                    .map(|hook| spawn_contract(hook, false)),
            )
            .collect()
    }
}

/// One hooks file, read as a session reads it under its project trust: what
/// [`HookCatalog::discover`] loads and `rho/hooks/list` (`configured.rs`)
/// lists, so the two cannot disagree on a file.
pub(super) struct HooksFileRead {
    pub(super) path: PathBuf,
    pub(super) outcome: HooksFileOutcome,
}

/// What reading one hooks file found.
pub(super) enum HooksFileOutcome {
    /// No file is there.
    Absent,
    /// A session loads its hooks.
    Loaded(Vec<HookDefinition>),
    /// The project's file, skipped for lack of trust: its hooks, or why it
    /// could not be read or parsed.
    Untrusted {
        definitions: Vec<HookDefinition>,
        error: Option<HookConfigError>,
    },
    /// A file a session would load could not be read or parsed, which stops
    /// it loading any hooks.
    Invalid(HookConfigError),
}

impl HooksFileRead {
    /// The user's `hooks.toml` under `rho_home`, which needs no trust.
    pub(super) fn user(rho_home: &Path) -> Self {
        Self::read(
            user_hooks_path(rho_home),
            HookSource::User,
            None,
            ProjectTrust::Trusted,
        )
    }

    /// The `.rho/hooks.toml` of the project at `root`, loaded under `trust`.
    pub(super) fn project(root: &Path, trust: ProjectTrust) -> Self {
        Self::read(
            root.join(".rho/hooks.toml"),
            HookSource::Project,
            Some(root),
            trust,
        )
    }

    fn read(
        path: PathBuf,
        source: HookSource,
        project_root: Option<&Path>,
        trust: ProjectTrust,
    ) -> Self {
        let outcome = match (
            read_definitions(&path, source, project_root),
            trust.is_trusted(),
        ) {
            (Ok(None), _) => HooksFileOutcome::Absent,
            (Ok(Some(hooks)), true) => HooksFileOutcome::Loaded(hooks),
            (Ok(Some(definitions)), false) => HooksFileOutcome::Untrusted {
                definitions,
                error: None,
            },
            (Err(error), true) => HooksFileOutcome::Invalid(error),
            (Err(error), false) => HooksFileOutcome::Untrusted {
                definitions: Vec::new(),
                error: Some(error),
            },
        };
        Self { path, outcome }
    }
}

/// What one hook will execute, shown to the user before trust is granted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSpawnContract {
    pub active: bool,
    pub id: String,
    pub(crate) event: ConfiguredHookEvent,
    pub tools: ToolMatcher,
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    pub timeout: std::time::Duration,
    pub environment: Vec<String>,
    /// Edit tools `tools` misses while it names others: a load-time
    /// problem that does not stop the hook from running.
    pub missing_edit_tools: Vec<&'static str>,
}

/// The user's hooks file of the Rho home `rho_home`.
pub(crate) fn user_hooks_path(rho_home: &Path) -> PathBuf {
    rho_home.join("hooks.toml")
}

/// Checks `text` as a session would load it from the user's hooks file at
/// `path`, before `rho/hooks/file/set` writes it there.
pub(crate) fn check_user_hooks(path: &Path, text: &str) -> Result<(), HookConfigError> {
    parse_hooks_file(path, text, HookSource::User, None, canonical_tool_names()).map(drop)
}

fn read_definitions(
    path: &Path,
    source: HookSource,
    project_root: Option<&Path>,
) -> Result<Option<Vec<HookDefinition>>, HookConfigError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(HookConfigError::at_file(
                path,
                format!("cannot read hooks file: {error}"),
            ));
        }
    };
    parse_hooks_file(
        path,
        &contents,
        source,
        project_root,
        canonical_tool_names(),
    )
    .map(Some)
}

pub(super) fn spawn_contract(hook: &HookDefinition, active: bool) -> HookSpawnContract {
    HookSpawnContract {
        active,
        id: hook.qualified_id(),
        event: hook.event(),
        tools: hook.tools().clone(),
        command: hook.command().to_vec(),
        working_directory: hook.working_directory().to_path_buf(),
        timeout: hook.timeout(),
        // The names `build_command` gives the child: the base set, Rho's own
        // marker, and the hook's allowlist.
        environment: crate::child_env::base_names()
            .chain([super::IN_HOOK_ENV])
            .map(str::to_owned)
            .chain(hook.env().iter().cloned())
            .collect(),
        missing_edit_tools: hook.tools().missing_edit_tools(),
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
