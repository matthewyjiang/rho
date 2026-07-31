//! Discovery, trust, and ordering of hook definitions.

use std::path::{Path, PathBuf};

use crate::tools::CANONICAL_TOOL_NAMES;
use rho_sdk::hooks::HookEventKind;

use super::{
    config::{parse_hooks_file, HookConfigError, HookDefinition},
    environment::base_environment_names,
    HookSource,
};

/// Environment variable that grants a workspace's project hooks.
///
/// Same family as `RHO_TRUST_PROJECT_AGENTS`: project-supplied executable policy
/// stays inert until the user says the workspace is trusted.
pub const TRUST_PROJECT_HOOKS_ENV: &str = "RHO_TRUST_PROJECT_HOOKS";

/// Whether a workspace's project hooks may load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectTrust {
    Trusted,
    Untrusted,
}

impl ProjectTrust {
    pub fn from_env(value: Option<&str>) -> Self {
        if value == Some("1") {
            Self::Trusted
        } else {
            Self::Untrusted
        }
    }
}

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
            catalog.load(&home.join("hooks.toml"), HookSource::User, None)?;
        }
        let Some(root) = project_root else {
            return Ok(catalog);
        };
        let project_file = root.join(".rho/hooks.toml");
        if trust == ProjectTrust::Untrusted {
            match read_definitions(&project_file, HookSource::Project, Some(root)) {
                Ok(Some(definitions)) => {
                    catalog.skipped_untrusted = Some(SkippedProjectHooks {
                        path: project_file,
                        definitions,
                        error: None,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    catalog.skipped_untrusted = Some(SkippedProjectHooks {
                        path: project_file,
                        definitions: Vec::new(),
                        error: Some(error),
                    });
                }
            }
            return Ok(catalog);
        }
        catalog.load(&project_file, HookSource::Project, Some(root))?;
        Ok(catalog)
    }

    fn load(
        &mut self,
        path: &Path,
        source: HookSource,
        project_root: Option<&Path>,
    ) -> Result<(), HookConfigError> {
        let Some(hooks) = read_definitions(path, source, project_root)? else {
            return Ok(());
        };
        self.files.push(path.to_path_buf());
        self.hooks.extend(hooks);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
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

/// What one hook will execute, rendered for the user before trust is granted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSpawnContract {
    pub active: bool,
    pub id: String,
    pub event: &'static str,
    pub tools: String,
    pub command: Vec<String>,
    pub working_directory: PathBuf,
    pub timeout: std::time::Duration,
    pub environment: Vec<String>,
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
            ))
        }
    };
    parse_hooks_file(path, &contents, source, project_root, CANONICAL_TOOL_NAMES).map(Some)
}

fn spawn_contract(hook: &HookDefinition, active: bool) -> HookSpawnContract {
    HookSpawnContract {
        active,
        id: hook.qualified_id(),
        event: hook.event().wire_name(),
        tools: hook.tools().describe(),
        command: hook.command().to_vec(),
        working_directory: hook.working_directory().to_path_buf(),
        timeout: hook.timeout(),
        environment: base_environment_names()
            .iter()
            .map(|name| (*name).to_owned())
            .chain(std::iter::once(super::IN_HOOK_ENV.to_owned()))
            .chain(hook.env().iter().cloned())
            .collect(),
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
