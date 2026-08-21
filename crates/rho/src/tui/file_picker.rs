use std::{
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use rho_tools::workspace_walk::{
    visit_files, HiddenFiles, WalkLimits, WalkOptions, WalkStop, MAX_ENTRIES_SCANNED,
};

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

/// Workspace paths discovered for `@` mentions, plus whether the walk finished.
#[derive(Clone, Debug)]
pub(super) struct DiscoveredFilePaths {
    pub(super) paths: Arc<Vec<String>>,
    /// True when discovery stopped early (deadline, entry cap, or result cap).
    pub(super) incomplete: bool,
}

/// One row the `@` palette can offer.
///
/// A mention can name something in the workspace or something a connected MCP
/// server holds. The two are told apart here rather than by inspecting a string,
/// because selecting them does entirely different things: one writes a path into
/// the message, the other pulls content into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FilePaletteEntry {
    WorkspaceFile(String),
    McpResource(crate::tools::mcp::McpResource),
}

/// The `@` palette's rows: matched server resources, then workspace paths.
///
/// The two sources stay in the lists they arrived in, and a row is built only
/// when something asks for it. Merging them into one vector of entries would
/// cost a heap allocation per path every time the query changes, and a bare `@`
/// on a large repository offers a hundred thousand paths that nobody scrolls to.
/// The workspace list here is the discovery cache's own `Arc`, shared rather
/// than copied.
#[derive(Clone, Debug)]
pub(super) struct FilePaletteMatches {
    resources: Arc<Vec<crate::tools::mcp::McpResource>>,
    paths: Arc<Vec<String>>,
    /// True when workspace discovery stopped early.
    pub(super) incomplete: bool,
}

impl FilePaletteMatches {
    pub(super) fn empty() -> Self {
        Self {
            resources: Arc::new(Vec::new()),
            paths: Arc::new(Vec::new()),
            incomplete: false,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.resources.len() + self.paths.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The row at `index`, counting resources first.
    pub(super) fn get(&self, index: usize) -> Option<FilePaletteEntry> {
        if let Some(resource) = self.resources.get(index) {
            return Some(FilePaletteEntry::McpResource(resource.clone()));
        }
        self.paths
            .get(index - self.resources.len())
            .cloned()
            .map(FilePaletteEntry::WorkspaceFile)
    }

    /// The rows from `start`, at most `count` of them, for a scrolled view.
    pub(super) fn rows(
        &self,
        start: usize,
        count: usize,
    ) -> impl Iterator<Item = (usize, FilePaletteEntry)> + '_ {
        (start..self.len())
            .take(count)
            .filter_map(|index| Some((index, self.get(index)?)))
    }
}

/// Rank the resources connected servers offer, then put them ahead of the
/// workspace files.
///
/// Resources lead because a workspace commonly holds thousands of files and a
/// server offers a handful. Appended, they would sit below a screenful of paths
/// and never be seen. The workspace order itself is left exactly as discovery
/// produced it.
///
/// Resources are matched on their URI, which is also what the palette shows and
/// what a template inserts, so what a person types lines up with what they read.
pub(super) fn file_palette_matches(
    discovered: DiscoveredFilePaths,
    resources: &[crate::tools::mcp::McpResource],
    query: &str,
) -> FilePaletteMatches {
    let keys = resources
        .iter()
        .map(|resource| resource.uri.as_str())
        .collect::<Vec<_>>();
    let matched = fuzzy_matching_indexes(&keys, query)
        .into_iter()
        .map(|index| resources[index].clone())
        .collect::<Vec<_>>();
    FilePaletteMatches {
        resources: Arc::new(matched),
        paths: discovered.paths,
        incomplete: discovered.incomplete,
    }
}

impl DiscoveredFilePaths {
    #[cfg(test)]
    fn complete(paths: Vec<String>) -> Self {
        Self {
            paths: Arc::new(paths),
            incomplete: false,
        }
    }

    pub(super) fn as_slice(&self) -> &[String] {
        self.paths.as_slice()
    }
}

/// The `@query` token under the cursor, if any.
///
/// Works on slices of `input` instead of collecting characters: the render
/// path calls this several times per frame, so nothing larger than the query
/// itself is ever copied.
pub(super) fn active_file_mention(input: &str, cursor: usize) -> Option<FileMention> {
    let cursor_byte = input
        .char_indices()
        .nth(cursor)
        .map_or(input.len(), |(byte, _)| byte);
    let (before, after) = input.split_at(cursor_byte);
    // The token is the last whitespace-delimited piece before the cursor plus
    // the piece after it up to the next whitespace.
    let head = before
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    let tail = after.split(char::is_whitespace).next().unwrap_or_default();
    let query = head.strip_prefix('@')?;
    if query.contains('@') {
        return None;
    }
    Some(FileMention {
        start: before.chars().count() - head.chars().count(),
        end: before.chars().count() + tail.chars().count(),
        query: query.to_string(),
    })
}

pub(super) fn matching_file_paths(cwd: &Path, query: &str) -> DiscoveredFilePaths {
    matching_file_paths_with_home(cwd, query, home_dir().as_deref())
}

#[cfg(test)]
pub(super) fn matching_file_paths_with_home_for_test(
    cwd: &Path,
    query: &str,
    home: Option<&Path>,
) -> DiscoveredFilePaths {
    matching_file_paths_with_home(cwd, query, home)
}

fn matching_file_paths_with_home(
    cwd: &Path,
    query: &str,
    home: Option<&Path>,
) -> DiscoveredFilePaths {
    let query = query.trim();
    if let Some((scope, residual)) = directory_scope(cwd, query, home) {
        let include_hidden = residual_includes_hidden(&residual);
        let discovered = file_paths_for_root(&scope.root, include_hidden);
        let matches = if residual.is_empty() {
            discovered.as_slice().to_vec()
        } else {
            fuzzy_matching_paths(discovered.as_slice(), &residual)
        };
        return DiscoveredFilePaths {
            paths: Arc::new(
                matches
                    .into_iter()
                    .map(|path| format!("{}{path}", scope.display_prefix))
                    .collect(),
            ),
            incomplete: discovered.incomplete,
        };
    }

    let include_hidden = residual_includes_hidden(query);
    let discovered = file_paths_for_root(cwd, include_hidden);
    if query.is_empty() {
        return discovered;
    }
    DiscoveredFilePaths {
        paths: Arc::new(fuzzy_matching_paths(discovered.as_slice(), query)),
        incomplete: discovered.incomplete,
    }
}

#[cfg(test)]
pub(super) fn workspace_file_paths(cwd: &Path) -> DiscoveredFilePaths {
    file_paths_for_root(cwd, /*include_hidden*/ false)
}

fn residual_includes_hidden(residual: &str) -> bool {
    residual.split('/').any(|part| part.starts_with('.'))
}

fn file_paths_for_root(root: &Path, include_hidden: bool) -> DiscoveredFilePaths {
    let root = normalize_existing_dir(root).unwrap_or_else(|| root.to_path_buf());
    let mut discovered = discover_file_paths(&root, include_hidden);
    Arc::make_mut(&mut discovered.paths).sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });
    discovered
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

#[cfg(test)]
fn path_to_unix_string(path: &Path) -> String {
    use std::path::Component;

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
    let keys = paths.iter().map(String::as_str).collect::<Vec<_>>();
    fuzzy_matching_indexes(&keys, query)
        .into_iter()
        .map(|index| paths[index].clone())
        .collect()
}

/// Rank `keys` against `query`, best first, returning the positions that
/// survived. Callers that carry more than a string per row map the positions
/// back onto their own rows.
///
/// An empty query keeps every key in its original order, which is what makes a
/// bare `@` list the workspace as discovered.
fn fuzzy_matching_indexes(keys: &[&str], query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..keys.len()).collect();
    }

    let mut matches = keys
        .iter()
        .enumerate()
        .filter_map(|(index, key)| fuzzy_match_score(key, query).map(|score| (index, score)))
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
    matches.into_iter().map(|(index, _)| index).collect()
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
    incomplete: bool,
) -> Option<String> {
    if above == 0 && below == 0 && !incomplete {
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
    if incomplete {
        parts.push("partial".into());
    }
    Some(parts.join(" · "))
}

/// Lists workspace files for `@` mentions using the shared workspace walker,
/// so ignore rules, symlink policy, and path shapes match the `grep` and
/// `glob` tools. Callers sort the result for display.
fn discover_file_paths(root: &Path, include_hidden: bool) -> DiscoveredFilePaths {
    let options = WalkOptions {
        hidden: if include_hidden {
            HiddenFiles::Include
        } else {
            HiddenFiles::Skip
        },
        limits: WalkLimits {
            max_entries: MAX_ENTRIES_SCANNED,
            deadline: Instant::now() + FILE_DISCOVERY_TIMEOUT,
        },
    };

    let mut paths = Vec::new();
    let stop = visit_files(root, &options, |file| {
        paths.push(file.relative);
        if paths.len() >= MAX_FILE_PATHS {
            ControlFlow::Break(WalkStop::ResultLimit)
        } else {
            ControlFlow::Continue(())
        }
    });
    DiscoveredFilePaths {
        paths: Arc::new(paths),
        incomplete: !matches!(stop, WalkStop::Completed),
    }
}

#[cfg(test)]
#[path = "file_picker_tests.rs"]
mod tests;
