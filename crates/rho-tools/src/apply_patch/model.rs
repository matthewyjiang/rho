//! Planned filesystem changes and their presentation metadata.

use std::path::{Path, PathBuf};

use crate::diff::unified_diff;

#[derive(Debug, Clone)]
pub(super) struct MoveSource {
    pub(super) path: PathBuf,
    pub(super) display_path: String,
    pub(super) content: String,
}

/// One planned filesystem operation. Impossible states are unrepresentable.
#[derive(Debug, Clone)]
pub(super) enum FileChange {
    Add {
        target: PathBuf,
        display_path: String,
        new_content: String,
    },
    Delete {
        target: PathBuf,
        display_path: String,
        previous_content: String,
        previous_permissions: std::fs::Permissions,
    },
    Update {
        target: PathBuf,
        display_path: String,
        old_content: String,
        new_content: String,
        permissions: std::fs::Permissions,
        move_from: Option<MoveSource>,
    },
}

impl FileChange {
    pub(super) fn summary_line(&self) -> String {
        match self {
            Self::Add { display_path, .. } => format!("A {display_path}"),
            Self::Delete { display_path, .. } => format!("D {display_path}"),
            Self::Update { display_path, .. } => format!("M {display_path}"),
        }
    }

    pub(super) fn affected_display_paths(&self) -> impl Iterator<Item = &str> {
        let paths = match self {
            Self::Update {
                display_path,
                move_from: Some(source),
                ..
            } => [
                Some(source.display_path.as_str()),
                Some(display_path.as_str()),
            ],
            Self::Add { display_path, .. }
            | Self::Delete { display_path, .. }
            | Self::Update { display_path, .. } => [Some(display_path.as_str()), None],
        };
        paths.into_iter().flatten()
    }

    pub(super) fn diff(&self) -> String {
        match self {
            Self::Add {
                display_path,
                new_content,
                ..
            } => unified_diff("", new_content, display_path, /*created*/ true),
            Self::Delete {
                display_path,
                previous_content,
                ..
            } => unified_diff(previous_content, "", display_path, /*created*/ false),
            Self::Update {
                display_path,
                old_content,
                new_content,
                ..
            } => unified_diff(
                old_content,
                new_content,
                display_path,
                /*created*/ false,
            ),
        }
    }

    pub(super) fn chain_snapshot(&self) -> Option<String> {
        match self {
            Self::Add {
                display_path,
                new_content,
                ..
            }
            | Self::Update {
                display_path,
                new_content,
                ..
            } => Some(crate::hashline::format_chain_snapshot_with(
                display_path,
                new_content,
                &[],
                /*mint_tag*/ false,
            )),
            Self::Delete { .. } => None,
        }
    }

    pub(super) fn write_target(&self) -> Option<(&Path, &str)> {
        match self {
            Self::Add {
                target,
                display_path,
                ..
            }
            | Self::Update {
                target,
                display_path,
                ..
            } => Some((target, display_path)),
            Self::Delete { .. } => None,
        }
    }

    pub(super) fn delete_target(&self) -> Option<(&Path, &str)> {
        match self {
            Self::Delete {
                target,
                display_path,
                ..
            } => Some((target, display_path)),
            Self::Update {
                move_from: Some(source),
                ..
            } => Some((&source.path, &source.display_path)),
            Self::Add { .. }
            | Self::Update {
                move_from: None, ..
            } => None,
        }
    }
}
