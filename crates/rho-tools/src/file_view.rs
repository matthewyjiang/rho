//! Live file-view style for read, write, and grep.
//!
//! Numbered `N:line` output is shared. Only [`crate::EditFormat::Hashline`]
//! wraps headers as `[path#TAG]`. Mid-session edit-tool switches update this
//! policy in place so sibling tools do not need to be rebuilt by name.

use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use crate::{
    hashline::{format_chain_snapshot, format_hashline_view, format_header, FileHash},
    text_view::{
        format_chain_snapshot as format_numbered_snapshot, format_numbered_view, read_text_window,
    },
    tool::ToolError,
    EditFormat,
};

/// How workspace file tools present a UTF-8 path to the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileViewStyle {
    /// Path header plus numbered lines. No snapshot fingerprint.
    Numbered,
    /// `[path#TAG]` header plus numbered lines for hashline `edit`.
    Hashline,
}

impl FileViewStyle {
    pub const fn from_edit_format(format: EditFormat) -> Self {
        if format.mints_snapshot_tags() {
            Self::Hashline
        } else {
            Self::Numbered
        }
    }

    pub const fn mints_snapshot_tags(self) -> bool {
        matches!(self, Self::Hashline)
    }

    pub(crate) fn format_view(
        self,
        display_path: &str,
        text: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String, String> {
        match self {
            Self::Numbered => format_numbered_view(display_path, text, offset, limit),
            Self::Hashline => format_hashline_view(display_path, text, offset, limit),
        }
    }

    pub(crate) fn format_chain_snapshot(
        self,
        display_path: &str,
        text: &str,
        focus_lines: &[usize],
    ) -> String {
        match self {
            Self::Numbered => format_numbered_snapshot(display_path, text, focus_lines),
            Self::Hashline => format_chain_snapshot(display_path, text, focus_lines),
        }
    }

    pub(crate) async fn read_window(
        self,
        path: &Path,
        display_path: &str,
        source_len: u64,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String, ToolError> {
        match self {
            Self::Numbered => {
                read_text_window(path, source_len, offset, limit, None::<FileHash>, |_| {
                    display_path.to_string()
                })
                .await
            }
            Self::Hashline => {
                read_text_window(
                    path,
                    source_len,
                    offset,
                    limit,
                    Some(FileHash::new()),
                    |tag| {
                        format_header(
                            display_path,
                            tag.expect("hashline window always fingerprints"),
                        )
                    },
                )
                .await
            }
        }
    }
}

/// Shared, updatable view style for the coding-tool suite.
#[derive(Clone, Debug)]
pub struct FileViewPolicy {
    format: Arc<RwLock<EditFormat>>,
}

impl FileViewPolicy {
    pub fn new(format: EditFormat) -> Self {
        Self {
            format: Arc::new(RwLock::new(format)),
        }
    }

    pub fn set(&self, format: EditFormat) {
        *self.write_format() = format;
    }

    pub fn current(&self) -> EditFormat {
        *self.read_format()
    }

    pub fn style(&self) -> FileViewStyle {
        FileViewStyle::from_edit_format(self.current())
    }

    fn read_format(&self) -> std::sync::RwLockReadGuard<'_, EditFormat> {
        self.format
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_format(&self) -> std::sync::RwLockWriteGuard<'_, EditFormat> {
        self.format
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for FileViewPolicy {
    fn default() -> Self {
        Self::new(EditFormat::default())
    }
}
