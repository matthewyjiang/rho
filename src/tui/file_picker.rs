use std::{
    path::{Component, Path},
    time::{Duration, Instant},
};

use ignore::WalkBuilder;

use super::{PickerAction, PickerItem, UiPicker};

const MAX_FILE_PATHS: usize = 100_000;
const FILE_DISCOVERY_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileMention {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) query: String,
}

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

pub(super) fn file_path_picker(cwd: &Path, filter: &str) -> UiPicker {
    let mut paths = discover_file_paths(cwd);
    paths.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });

    let items = paths
        .into_iter()
        .map(|path| PickerItem {
            label: path.clone(),
            detail: None,
            preview: None,
            badge: None,
            value: path,
        })
        .collect();
    let mut picker = UiPicker::new(
        "workspace files",
        "type fuzzy search, enter inserts, tab insert, esc cancel",
        items,
        PickerAction::InsertFilePath,
    );
    picker.filter = filter.to_string();
    picker.select_first_match();
    picker
}

fn discover_file_paths(cwd: &Path) -> Vec<String> {
    walk_file_paths(cwd)
}

fn walk_file_paths(cwd: &Path) -> Vec<String> {
    let deadline = Instant::now() + FILE_DISCOVERY_TIMEOUT;
    let mut builder = WalkBuilder::new(cwd);
    builder
        .hidden(/*yes*/ false)
        .follow_links(/*yes*/ false)
        .filter_entry(|entry| entry.file_name() != ".git");

    builder
        .build()
        .take_while(|_| Instant::now() < deadline)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| display_workspace_path(cwd, entry.path()))
        .take(MAX_FILE_PATHS)
        .collect()
}

fn display_workspace_path(cwd: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(cwd).ok()?;
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
