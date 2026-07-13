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
pub(super) const FILE_PATH_CACHE_TTL: Duration = Duration::from_secs(2);
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePathCacheKey {
    root: PathBuf,
    include_hidden: bool,
}

#[derive(Debug)]
struct FilePathCache {
    key: Option<FilePathCacheKey>,
    paths: Arc<Vec<String>>,
    cached_at: Instant,
}

static FILE_PATH_CACHE: LazyLock<Mutex<FilePathCache>> = LazyLock::new(|| {
    Mutex::new(FilePathCache {
        key: None,
        paths: Arc::new(Vec::new()),
        cached_at: Instant::now(),
    })
});

pub(super) fn active_file_mention(input: &str, cursor: usize) -> Option<FileMention> {
    let chars = input.chars().collect::<Vec<_>>();
    let cursor = cursor.min(chars.len());
    let start = chars[..cursor]
        .iter()
        .rposition(|ch| ch.is_whitespace())
        .map_or(0, |index| index + 1);
    let token_prefix = chars[start..cursor].iter().collect::<String>();
    let query = token_prefix.strip_prefix('@')?;
    if query.contains('@') {
        return None;
    }
    let end = chars[cursor..]
        .iter()
        .position(|ch| ch.is_whitespace())
        .map_or(chars.len(), |offset| cursor + offset);

    Some(FileMention {
        start,
        end,
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
        let include_hidden = residual_includes_hidden(&residual);
        let relative_paths = file_paths_for_root(&scope.root, include_hidden);
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

    let include_hidden = residual_includes_hidden(query);
    let paths = file_paths_for_root(cwd, include_hidden);
    if query.is_empty() {
        return paths;
    }
    Arc::new(fuzzy_matching_paths(paths.as_slice(), query))
}

#[cfg(test)]
pub(super) fn workspace_file_paths(cwd: &Path) -> Arc<Vec<String>> {
    file_paths_for_root(cwd, /*include_hidden*/ false)
}

fn residual_includes_hidden(residual: &str) -> bool {
    residual.split('/').any(|part| part.starts_with('.'))
}

fn file_paths_for_root(root: &Path, include_hidden: bool) -> Arc<Vec<String>> {
    let root = normalize_existing_dir(root).unwrap_or_else(|| root.to_path_buf());
    let key = FilePathCacheKey {
        root: root.clone(),
        include_hidden,
    };
    if let Ok(cache) = FILE_PATH_CACHE.lock() {
        if cache.key.as_ref() == Some(&key) && cache.cached_at.elapsed() < FILE_PATH_CACHE_TTL {
            return Arc::clone(&cache.paths);
        }
    }

    let mut paths = discover_file_paths(&root, include_hidden);
    paths.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });
    let paths = Arc::new(paths);

    if let Ok(mut cache) = FILE_PATH_CACHE.lock() {
        cache.key = Some(key);
        cache.paths = Arc::clone(&paths);
        cache.cached_at = Instant::now();
    }
    paths
}

#[cfg(test)]
pub(super) fn clear_workspace_file_path_cache() {
    if let Ok(mut cache) = FILE_PATH_CACHE.lock() {
        cache.key = None;
        cache.paths = Arc::new(Vec::new());
        cache.cached_at = Instant::now();
    }
}

#[cfg(test)]
pub(super) fn expire_workspace_file_path_cache() {
    if let Ok(mut cache) = FILE_PATH_CACHE.lock() {
        cache.cached_at = Instant::now() - FILE_PATH_CACHE_TTL;
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
    let display_prefix = directory_display_prefix(directory_query);
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

fn directory_display_prefix(directory_query: &str) -> String {
    if directory_query == "/" {
        "/".into()
    } else {
        format!("{directory_query}/")
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

fn discover_file_paths(root: &Path, include_hidden: bool) -> Vec<String> {
    walk_file_paths(root, include_hidden)
}

fn walk_file_paths(root: &Path, include_hidden: bool) -> Vec<String> {
    let deadline = Instant::now() + FILE_DISCOVERY_TIMEOUT;
    let mut builder = WalkBuilder::new(root);
    // Always allow walking into an explicitly scoped hidden root (depth 0).
    // Hidden children are controlled by filter_entry below.
    builder
        .hidden(/*yes*/ false)
        .follow_links(/*yes*/ false)
        .filter_entry(move |entry| {
            let name = entry.file_name();
            if name == ".git" {
                return false;
            }
            if include_hidden || entry.depth() == 0 {
                return true;
            }
            !name.to_string_lossy().starts_with('.')
        });

    builder
        .build()
        .take_while(|_| Instant::now() < deadline)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let path = display_relative_path(root, entry.path())?;
            if !include_hidden && path_has_hidden_component(&path) {
                return None;
            }
            Some(path)
        })
        .take(MAX_FILE_PATHS)
        .collect()
}

fn path_has_hidden_component(path: &str) -> bool {
    path.split('/').any(|part| part.starts_with('.'))
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
