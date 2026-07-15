use std::path::{Path, PathBuf};

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
}
