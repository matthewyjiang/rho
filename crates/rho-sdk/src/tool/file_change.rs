use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Complete UTF-8 file contents before and after a successful tool mutation.
///
/// This is host-facing presentation data, not model output or a unified patch.
/// Producers should use the resolved absolute path and the text actually read
/// and committed by the mutation, rather than reading the file again afterward.
/// Clones share the text so event subscribers do not copy entire files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolFileChange {
    path: PathBuf,
    old_text: Option<Arc<str>>,
    new_text: Arc<str>,
}

impl ToolFileChange {
    /// A newly created file. Its original contents are absent, not empty.
    pub fn created(path: impl Into<PathBuf>, new_text: impl Into<Arc<str>>) -> Self {
        Self {
            path: path.into(),
            old_text: None,
            new_text: new_text.into(),
        }
    }

    /// An existing file rewritten with complete replacement contents.
    pub fn modified(
        path: impl Into<PathBuf>,
        old_text: impl Into<Arc<str>>,
        new_text: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            path: path.into(),
            old_text: Some(old_text.into()),
            new_text: new_text.into(),
        }
    }

    /// A deleted file, represented by empty replacement contents.
    pub fn deleted(path: impl Into<PathBuf>, old_text: impl Into<Arc<str>>) -> Self {
        Self::modified(path, old_text, "")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` means the file did not exist before the mutation.
    pub fn old_text(&self) -> Option<&str> {
        self.old_text.as_deref()
    }

    pub fn new_text(&self) -> &str {
        &self.new_text
    }
}
