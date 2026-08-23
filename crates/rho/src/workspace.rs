use std::path::{Path, PathBuf};

/// Environment variable that grants a workspace's project hooks.
pub const TRUST_PROJECT_HOOKS_ENV: &str = "RHO_TRUST_PROJECT_HOOKS";
/// Environment variable that grants a workspace's project agent definitions.
pub const TRUST_PROJECT_AGENTS_ENV: &str = "RHO_TRUST_PROJECT_AGENTS";
/// Environment variable that grants a workspace's project Agent Plugins.
pub const TRUST_PROJECT_PLUGINS_ENV: &str = "RHO_TRUST_PROJECT_PLUGINS";

/// Whether a workspace's project-supplied files may activate.
///
/// Shared by project hooks, project agents, and project Agent Plugins.
/// Each feature has its own env var; only the exact value `1` grants trust.
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

    pub fn from_env_var(name: &str) -> Self {
        Self::from_env(std::env::var(name).ok().as_deref())
    }

    pub fn from_hooks_env() -> Self {
        Self::from_env_var(TRUST_PROJECT_HOOKS_ENV)
    }

    pub fn from_agents_env() -> Self {
        Self::from_env_var(TRUST_PROJECT_AGENTS_ENV)
    }

    pub fn from_plugins_env() -> Self {
        Self::from_env_var(TRUST_PROJECT_PLUGINS_ENV)
    }

    pub const fn is_trusted(self) -> bool {
        matches!(self, Self::Trusted)
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

    #[test]
    fn returns_git_root_through_cwd() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let child = repo.path().join("src/nested");
        std::fs::create_dir_all(&child).unwrap();

        let dirs = project_ancestor_dirs(&child);

        assert_eq!(
            dirs,
            vec![repo.path().to_path_buf(), repo.path().join("src"), child]
        );
    }

    #[test]
    fn returns_only_cwd_outside_git_worktree() {
        let dir = TempDir::new().unwrap();
        let child = dir.path().join("src");
        std::fs::create_dir_all(&child).unwrap();

        let dirs = project_ancestor_dirs(&child);

        assert_eq!(dirs, vec![child]);
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
