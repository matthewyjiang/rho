use std::path::{Path, PathBuf};

/// Environment variable that grants a workspace's project hooks.
pub const TRUST_PROJECT_HOOKS_ENV: &str = "RHO_TRUST_PROJECT_HOOKS";
/// Environment variable that grants a workspace's project agent definitions.
pub const TRUST_PROJECT_AGENTS_ENV: &str = "RHO_TRUST_PROJECT_AGENTS";
/// Environment variable that grants a workspace's project Agent Plugins.
pub const TRUST_PROJECT_PLUGINS_ENV: &str = "RHO_TRUST_PROJECT_PLUGINS";
/// Environment variable that grants a workspace's project OpenAPI sources.
pub const TRUST_PROJECT_OPENAPI_ENV: &str = "RHO_TRUST_PROJECT_OPENAPI";

/// Whether a workspace's project-supplied files may activate.
///
/// Shared by project hooks, agents, Agent Plugins, and OpenAPI sources.
/// Each feature has its own env var; only the exact value `1` grants trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectTrust {
    Trusted,
    Untrusted,
}

impl ProjectTrust {
    fn from_env(value: Option<&str>) -> Self {
        if value == Some("1") {
            Self::Trusted
        } else {
            Self::Untrusted
        }
    }

    pub const fn is_trusted(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

/// A workspace's [`ProjectTrust`] for each feature that asks for it, as one
/// environment grants them.
///
/// Read once from the environment of whoever opens a session or starts a
/// workflow run, and kept for as long as that runs: a local `rho`'s own
/// environment, or, in the daemon, the client's (#1779). Nothing reads the
/// variables later, so what a session loaded when it opened and what it
/// loads afterwards agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectTrusts {
    pub hooks: ProjectTrust,
    pub agents: ProjectTrust,
    pub plugins: ProjectTrust,
    pub openapi: ProjectTrust,
}

impl ProjectTrusts {
    /// No trust: what an environment that sets none of the variables grants.
    pub const UNTRUSTED: Self = Self {
        hooks: ProjectTrust::Untrusted,
        agents: ProjectTrust::Untrusted,
        plugins: ProjectTrust::Untrusted,
        openapi: ProjectTrust::Untrusted,
    };

    /// The trust the environment `read` reads grants.
    pub fn read(read: impl Fn(&str) -> Option<String>) -> Self {
        let trust = |name| ProjectTrust::from_env(read(name).as_deref());
        Self {
            hooks: trust(TRUST_PROJECT_HOOKS_ENV),
            agents: trust(TRUST_PROJECT_AGENTS_ENV),
            plugins: trust(TRUST_PROJECT_PLUGINS_ENV),
            openapi: trust(TRUST_PROJECT_OPENAPI_ENV),
        }
    }

    /// The trust this process's environment grants, for a command that acts
    /// for the terminal it runs in. A daemon session takes its client's
    /// instead (see [`crate::session_environment::SessionEnvironment`]).
    pub fn of_this_process() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }
}

pub fn project_ancestor_dirs(cwd: &Path) -> Vec<PathBuf> {
    let ancestors: Vec<_> = cwd.ancestors().map(Path::to_path_buf).collect();
    let Some(root_index) = ancestors.iter().position(|path| path.join(".git").exists()) else {
        return vec![cwd.to_path_buf()];
    };

    ancestors[..=root_index].iter().rev().cloned().collect()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    // Covers: project ancestor dirs run from the git root down to cwd, and are
    // just cwd outside a repository.
    // Owner: workspace root detection
    #[test]
    fn ancestor_dirs_start_at_the_git_root_or_only_cwd_outside_a_repo() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let child = repo.path().join("src/nested");
        std::fs::create_dir_all(&child).unwrap();

        let dirs = project_ancestor_dirs(&child);

        assert_eq!(
            dirs,
            vec![repo.path().to_path_buf(), repo.path().join("src"), child]
        );

        let outside = TempDir::new().unwrap();
        let child = outside.path().join("src");
        std::fs::create_dir_all(&child).unwrap();
        assert_eq!(
            project_ancestor_dirs(&child),
            vec![child],
            "outside a git worktree only cwd is returned"
        );
    }

    // Covers: only the exact value `1` grants trust, matching the
    // RHO_TRUST_PROJECT_* family contract used by hooks, agents, and plugins.
    // Owner: workspace trust policy.
    #[test]
    fn project_trust_requires_exact_opt_in() {
        assert_eq!(ProjectTrust::from_env(Some("1")), ProjectTrust::Trusted);
        for value in [Some("0"), Some("true"), Some("yes"), Some(""), None] {
            assert_eq!(
                ProjectTrust::from_env(value),
                ProjectTrust::Untrusted,
                "{value:?}"
            );
        }
    }
}
