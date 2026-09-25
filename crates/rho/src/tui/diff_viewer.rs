//! `/diff` popup: changed files on the left, the selected file's patch on the
//! right.
//!
//! Opening runs only the cheap listing ([`local_diff::collect_status`]).
//! Patches load one file at a time when a row is selected. Git and the patch
//! parse run on a blocking task, so large worktrees open instantly. At most
//! one load runs; its completion loads whatever is selected by then, which
//! naturally debounces held arrow keys. Loaded patches stay on their picker
//! rows, so revisiting a file is free.
//!
//! Picker rows are built in file order and filtering never reorders
//! `items`, so an item index is also the file's index in [`DiffViewer`].

use std::{path::PathBuf, sync::Arc};

use rho_tools::tool_card::DiffRow;
use tokio::task::JoinHandle;

use super::{
    diff_pane::{patch_rows, row_stats},
    local_diff::{self, ChangedFile, DiffSection, FileChange, WorktreeStatus},
    picker::{
        DetailBlock, DetailSheet, DiffDetail, LabelOverflow, OverlayChrome, OverlayShape,
        PickerDetail,
    },
    App, ComposerMode, Entry, PickerBadge, PickerBadgeTone, PickerItem, PickerKeyHints,
    PickerLayout, TabKey, UiPicker,
};

/// Open viewer state kept beside its picker.
#[derive(Debug)]
pub(super) struct DiffViewer {
    repo_root: PathBuf,
    /// One slot per picker item, same order.
    slots: Vec<FileSlot>,
    /// In-flight patch load and the file index it is for.
    pending: Option<(usize, JoinHandle<anyhow::Result<Vec<DiffRow>>>)>,
}

#[derive(Debug)]
struct FileSlot {
    file: ChangedFile,
    /// Whether the picker row already holds this file's patch (or its error).
    loaded: bool,
}

impl App {
    pub(super) fn execute_diff_command(&mut self) -> anyhow::Result<()> {
        let status = match local_diff::collect_status(&self.info.runtime.cwd) {
            Ok(status) => status,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not show git diff: {error}")));
                self.set_status("git diff unavailable");
                return Ok(());
            }
        };
        if status.files.is_empty() {
            self.set_status("worktree clean");
            return Ok(());
        }
        self.input_ui
            .set_composer(ComposerMode::Picker(diff_picker(&status)));
        self.diff_viewer = Some(DiffViewer {
            repo_root: status.repo_root,
            slots: status
                .files
                .into_iter()
                .map(|file| FileSlot {
                    file,
                    loaded: false,
                })
                .collect(),
            pending: None,
        });
        self.set_status("worktree diff");
        self.request_selected_diff();
        Ok(())
    }

    /// Start loading the selected file's patch unless it is loaded or another
    /// load is running (that one's completion calls back here).
    pub(super) fn request_selected_diff(&mut self) {
        let Some(index) = self.selected_diff_index() else {
            return;
        };
        let Some(viewer) = self.diff_viewer.as_mut() else {
            return;
        };
        let Some(slot) = viewer.slots.get(index) else {
            return;
        };
        if viewer.pending.is_some() || slot.loaded {
            return;
        }
        let repo_root = viewer.repo_root.clone();
        let file = slot.file.clone();
        let handle = tokio::task::spawn_blocking(move || {
            local_diff::file_patch(&repo_root, &file).map(|patch| patch_rows(&patch))
        });
        viewer.pending = Some((index, handle));
    }

    pub(super) fn diff_load_pending(&self) -> bool {
        self.diff_viewer
            .as_ref()
            .is_some_and(|viewer| viewer.pending.is_some())
    }

    pub(super) fn diff_load_finished(&self) -> bool {
        self.diff_viewer.as_ref().is_some_and(|viewer| {
            viewer
                .pending
                .as_ref()
                .is_some_and(|(_, handle)| handle.is_finished())
        })
    }

    /// Apply a finished patch load. Drops viewer state once its picker is
    /// gone; a load still running then finishes unobserved on its thread.
    pub(super) async fn poll_diff_viewer(&mut self) -> bool {
        if self.diff_viewer.is_none() {
            return false;
        }
        if !matches!(self.input_ui.composer(), ComposerMode::Picker(picker) if picker.is_view_diff())
        {
            self.diff_viewer = None;
            return false;
        }
        if !self.diff_load_finished() {
            return false;
        }
        let Some((index, handle)) = self
            .diff_viewer
            .as_mut()
            .and_then(|viewer| viewer.pending.take())
        else {
            return false;
        };
        let result = handle
            .await
            .unwrap_or_else(|error| Err(anyhow::anyhow!("patch load failed: {error}")));
        self.apply_loaded_patch(index, result);
        self.request_selected_diff();
        true
    }

    fn selected_diff_index(&self) -> Option<usize> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_view_diff() => picker.selected_index(),
            _ => None,
        }
    }

    fn apply_loaded_patch(&mut self, index: usize, result: anyhow::Result<Vec<DiffRow>>) {
        let Some(slot) = self
            .diff_viewer
            .as_mut()
            .and_then(|viewer| viewer.slots.get_mut(index))
        else {
            return;
        };
        slot.loaded = true;
        let file = &slot.file;
        let ComposerMode::Picker(picker) = self.input_ui.composer_mut() else {
            return;
        };
        let Some(item) = picker.items.get_mut(index) else {
            return;
        };
        item.detail = Some(match result {
            Ok(rows) => {
                // Untracked files are not in numstat; count them now.
                if file.change == FileChange::Untracked {
                    item.badge = Some(file_badge(file, Some(row_stats(&rows))));
                }
                PickerDetail::Diff(Arc::new(DiffDetail {
                    heading: file_heading(file),
                    path: file.path.clone(),
                    rows,
                }))
            }
            Err(error) => PickerDetail::Sheet(DetailSheet {
                blocks: vec![
                    DetailBlock::Title {
                        text: file_heading(file),
                        tag: None,
                    },
                    DetailBlock::Error(format!("could not load diff: {error}")),
                ],
            }),
        });
    }
}

fn diff_picker(status: &WorktreeStatus) -> UiPicker {
    let branch = status.branch.as_deref().unwrap_or("detached HEAD");
    let count = status.files.len();
    let noun = if count == 1 { "file" } else { "files" };
    let items = status.files.iter().map(file_item).collect();
    UiPicker::view_diff(format!("Diff · {branch} · {count} {noun}"), items)
        .with_layout(PickerLayout::Overlay)
        .with_overlay_shape(OverlayShape::Viewer)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " FILES".into(),
            detail_label: Some(" DIFF".into()),
            nav_keys_hint: "↑↓ files".into(),
        })
        .with_label_overflow(LabelOverflow::KeepEnd)
        .with_key_hints(PickerKeyHints {
            tab: TabKey::CycleItems,
            ..PickerKeyHints::default()
        })
        .with_fuzzy_filter()
}

fn file_item(file: &ChangedFile) -> PickerItem {
    PickerItem {
        label: file.path.clone(),
        section: Some(section_label(file.section).into()),
        detail: Some(PickerDetail::Text("loading diff…".into())),
        preview: None,
        badge: Some(file_badge(file, file.stats)),
        // The filter also matches `value`; keep it the path so typing only
        // ever matches what the row shows.
        value: file.path.clone(),
        selection_verb: None,
        allow_filter_completion: false,
    }
}

fn section_label(section: DiffSection) -> &'static str {
    match section {
        DiffSection::Staged => "Staged",
        DiffSection::Unstaged => "Unstaged",
        DiffSection::Untracked => "Untracked",
    }
}

/// `M +12 -3`: git's status letter, then line counts when known.
fn file_badge(file: &ChangedFile, stats: Option<(u64, u64)>) -> PickerBadge {
    let (marker, tone) = match file.change {
        FileChange::Modified => ("M", PickerBadgeTone::Muted),
        FileChange::Added => ("A", PickerBadgeTone::Healthy),
        FileChange::Deleted => ("D", PickerBadgeTone::Error),
        FileChange::Renamed => ("R", PickerBadgeTone::Muted),
        FileChange::Copied => ("C", PickerBadgeTone::Muted),
        FileChange::TypeChanged => ("T", PickerBadgeTone::Muted),
        FileChange::Unmerged => ("U", PickerBadgeTone::Warning),
        FileChange::Untracked => ("??", PickerBadgeTone::Healthy),
    };
    let text = match stats {
        Some((added, removed)) => format!("{marker} +{added} -{removed}"),
        None => marker.to_string(),
    };
    PickerBadge { text, tone }
}

fn file_heading(file: &ChangedFile) -> String {
    match &file.orig_path {
        Some(orig) => format!("{orig} → {}", file.path),
        None => file.path.clone(),
    }
}
