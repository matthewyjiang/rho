use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn git_branch(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/")
        .map(ToString::to_string)
        .or_else(|| head.get(..7).map(ToString::to_string))
}

pub(super) fn git_remote_urls(cwd: &Path) -> Vec<String> {
    let Some(git_dir) = find_git_dir(cwd) else {
        return Vec::new();
    };
    let config_path = git_common_dir(&git_dir).join("config");
    let Ok(config) = fs::read_to_string(config_path) else {
        return Vec::new();
    };
    remote_urls_from_config(&config)
}

fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let contents = fs::read_to_string(&dot_git).ok()?;
            let path = contents.trim().strip_prefix("gitdir: ")?;
            let path = Path::new(path);
            return Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                dir.join(path)
            });
        }
    }
    None
}

fn git_common_dir(git_dir: &Path) -> PathBuf {
    let Ok(contents) = fs::read_to_string(git_dir.join("commondir")) else {
        return git_dir.to_path_buf();
    };
    let path = Path::new(contents.trim());
    if path.as_os_str().is_empty() {
        return git_dir.to_path_buf();
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        git_dir.join(path)
    }
}

fn remote_urls_from_config(config: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut in_remote = false;
    for line in config.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            in_remote = rest
                .strip_suffix(']')
                .is_some_and(|section| remote_section_name(section).is_some());
            continue;
        }
        if !in_remote {
            continue;
        }
        let Some(value) = line.strip_prefix("url") else {
            continue;
        };
        let Some(value) = value.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = unquote_config_value(value.trim());
        if !value.is_empty() {
            urls.push(value.to_string());
        }
    }
    urls
}

fn remote_section_name(section: &str) -> Option<&str> {
    let section = section.trim();
    let name = section.strip_prefix("remote ")?.trim();
    Some(unquote_config_value(name))
}

fn unquote_config_value(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
