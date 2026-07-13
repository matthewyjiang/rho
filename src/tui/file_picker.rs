use std::{
    path::{Component, Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use ignore::WalkBuilder;

use super::picker::fuzzy_match_score;
use crate::paths::home_dir;

const MAX_FILE_PATHS: usize = 100_000;
const FILE_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(750);
/// Keep navigation bounded so weak queries stay interactive in large repos.
const MAX_RANKED_FILE_MATCHES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileMention {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) query: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DirectoryScope {
    root: PathBuf,
    display_prefix: String,
}

#[derive(Debug)]
struct FilePathCache {
    root: Option<PathBuf>,
    paths: Arc<Vec<String>>,
}

static FILE_PATH_CACHE: LazyLock<Mutex<FilePathCache>> = LazyLock::new(|| {
    Mutex::new(FilePathCache {
        root: None,
        paths: Arc::new(Vec::new()),
    })
});

pub(super) fn active_file_mention(input: &str, cursor: usize) -> Option<FileMention> {
    let chars = input.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|ch| ch.is_whitespace())
        .map_or(0, |index| index + 1);
    let token = chars[start..cursor].iter().collect::<String>();
    let query = token.strip_prefix('@')?;
    if query.contains('@') {
        return None;
    }

    Some(FileMention {
        start,
        end: cursor,
        query: query.to_string(),
    })
}

pub(super) fn matching_file_paths(cwd: &Path, query: &str) -> Arc<Vec<String>> {
    matching_file_paths_with_home(cwd, query, home_dir().as_deref())
}

#[cfg(test)]
pub(super) fn matching_file_paths_with_home_for_test(
    cwd: &Path,
    query: &str,
    home: Option<&Path>,
) -> Arc<Vec<String>> {
    matching_file_paths_with_home(cwd, query, home)
}

fn matching_file_paths_with_home(cwd: &Path, query: &str, home: Option<&Path>) -> Arc<Vec<String>> {
    let query = query.trim();
    if let Some((scope, residual)) = directory_scope(cwd, query, home) {
        let relative_paths = file_paths_for_root(&scope.root);
        let matches = if residual.is_empty() {
            relative_paths.as_slice().to_vec()
        } else {
            fuzzy_matching_paths(relative_paths.as_slice(), &residual)
        };
        return Arc::new(
            matches
                .into_iter()
                .map(|path| format!("{}{path}", scope.display_prefix))
                .collect(),
        );
    }

    let paths = file_paths_for_root(cwd);
    if query.is_empty() {
        return paths;
    }
    Arc::new(fuzzy_matching_paths(paths.as_slice(), query))
}

pub(super) fn workspace_file_paths(cwd: &Path) -> Arc<Vec<String>> {
    file_paths_for_root(cwd)
}

fn file_paths_for_root(root: &Path) -> Arc<Vec<String>> {
    let root = normalize_existing_dir(root).unwrap_or_else(|| root.to_path_buf());
    if let Ok(cache) = FILE_PATH_CACHE.lock() {
        if cache.root.as_ref() == Some(&root) {
            return Arc::clone(&cache.paths);
        }
    }

    let mut paths = discover_file_paths(&root);
    paths.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });
    let paths = Arc::new(paths);

    if let Ok(mut cache) = FILE_PATH_CACHE.lock() {
        cache.root = Some(root);
        cache.paths = Arc::clone(&paths);
    }
    paths
}

#[cfg(test)]
pub(super) fn clear_workspace_file_path_cache() {
    if let Ok(mut cache) = FILE_PATH_CACHE.lock() {
        cache.root = None;
        cache.paths = Arc::new(Vec::new());
    }
}

fn directory_scope(
    cwd: &Path,
    query: &str,
    home: Option<&Path>,
) -> Option<(DirectoryScope, String)> {
    if query.is_empty() || !query.contains('/') {
        return None;
    }

    let (directory_query, residual) = if query.ends_with('/') {
        (query.trim_end_matches('/'), "")
    } else {
        let (directory, residual) = query.rsplit_once('/')?;
        (directory, residual)
    };

    // Bare "@/" is treated as filesystem root scope.
    let directory_query = if directory_query.is_empty() {
        "/"
    } else {
        directory_query
    };

    let root = resolve_user_path(cwd, directory_query, home);
    let root = normalize_existing_dir(&root)?;
    let display_prefix = display_dir_prefix(cwd, &root, home);
    Some((
        DirectoryScope {
            root,
            display_prefix,
        },
        residual.to_string(),
    ))
}

fn resolve_user_path(cwd: &Path, path: &str, home: Option<&Path>) -> PathBuf {
    if path == "~" {
        return home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path));
    }

    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

fn normalize_existing_dir(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    path.is_dir().then_some(path)
}

fn display_dir_prefix(cwd: &Path, dir: &Path, home: Option<&Path>) -> String {
    // Prefer home-relative display for ~/ scopes even when a cwd-relative path exists.
    if let Some(home) = home.and_then(|home| home.canonicalize().ok()) {
        if let Ok(relative) = dir.strip_prefix(&home) {
            let relative = path_to_unix_string(relative);
            if relative.is_empty() {
                return "~/".into();
            }
            return format!("~/{relative}/");
        }
    }

    if let Some(relative) = relative_path(cwd, dir) {
        if relative == "." {
            return String::new();
        }
        return format!("{relative}/");
    }

    let mut absolute = path_to_unix_string(dir);
    if !absolute.ends_with('/') {
        absolute.push('/');
    }
    absolute
}

fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let from = from.canonicalize().ok()?;
    let to = to.canonicalize().ok()?;
    let from_components = from.components().collect::<Vec<_>>();
    let to_components = to.components().collect::<Vec<_>>();

    let mut shared = 0;
    while shared < from_components.len()
        && shared < to_components.len()
        && from_components[shared] == to_components[shared]
    {
        shared += 1;
    }

    let mut parts = Vec::new();
    for _ in shared..from_components.len() {
        parts.push(String::from(".."));
    }
    for component in &to_components[shared..] {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if parts.is_empty() {
        Some(String::from("."))
    } else {
        Some(parts.join("/"))
    }
}

fn path_to_unix_string(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => parts.push(String::new()),
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => parts.push(String::from("..")),
            Component::Prefix(prefix) => {
                parts.push(prefix.as_os_str().to_string_lossy().into_owned())
            }
        }
    }
    if parts.len() == 1 && parts[0].is_empty() {
        "/".into()
    } else {
        parts.join("/")
    }
}

pub(super) fn fuzzy_matching_paths(paths: &[String], query: &str) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() {
        return paths.to_vec();
    }

    let mut matches = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| fuzzy_match_score(path, query).map(|score| (index, score)))
        .collect::<Vec<_>>();

    if matches.len() > MAX_RANKED_FILE_MATCHES {
        matches.select_nth_unstable_by(MAX_RANKED_FILE_MATCHES - 1, |left, right| {
            right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
        });
        matches.truncate(MAX_RANKED_FILE_MATCHES);
    }

    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_index.cmp(right_index))
    });
    matches
        .into_iter()
        .map(|(index, _)| paths[index].clone())
        .collect()
}

pub(super) fn file_palette_scroll_counts(
    match_count: usize,
    selected_index: usize,
    visible_rows: usize,
) -> (usize, usize, usize) {
    if match_count == 0 || visible_rows == 0 {
        return (0, 0, 0);
    }

    let selected_index = selected_index.min(match_count - 1);
    let start = selected_index
        .saturating_add(1)
        .saturating_sub(visible_rows)
        .min(match_count.saturating_sub(1));
    let visible = visible_rows.min(match_count.saturating_sub(start));
    let above = start;
    let below = match_count.saturating_sub(start + visible);
    (start, above, below)
}

pub(super) fn file_palette_scroll_footer(
    above: usize,
    below: usize,
    total: usize,
) -> Option<String> {
    if above == 0 && below == 0 {
        return None;
    }

    let mut parts = Vec::new();
    if above > 0 {
        parts.push(format!("↑ {above} more"));
    }
    if below > 0 {
        parts.push(format!("↓ {below} more"));
    }
    parts.push(format!("{total} total"));
    Some(parts.join(" · "))
}

fn discover_file_paths(root: &Path) -> Vec<String> {
    walk_file_paths(root)
}

fn walk_file_paths(root: &Path) -> Vec<String> {
    let deadline = Instant::now() + FILE_DISCOVERY_TIMEOUT;
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(/*yes*/ false)
        .follow_links(/*yes*/ false)
        .filter_entry(|entry| entry.file_name() != ".git");

    builder
        .build()
        .take_while(|_| Instant::now() < deadline)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| display_relative_path(root, entry.path()))
        .take(MAX_FILE_PATHS)
        .collect()
}

fn display_relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
#[path = "file_picker_tests.rs"]
mod tests;
