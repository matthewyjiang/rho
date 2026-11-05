//! Coding tool selection and SDK registry construction.

use std::sync::Arc;

use rho_sdk::tool::Tool;

use crate::{
    sdk_search::{GlobTool, GrepTool},
    EditFormat, DEFAULT_MAX_OUTPUT_BYTES,
};

use super::{ListDirTool, ReadFileTool, WriteFileTool};

/// Options for coding tools registered on an SDK runtime.
#[derive(Clone)]
pub struct CodingToolOptions {
    max_output_bytes: usize,
    mutation_observer: Option<Arc<dyn crate::WorkspaceMutationObserver>>,
    edit_tool: EditFormat,
    host_files: Option<Arc<dyn crate::HostTextFiles>>,
}

impl Default for CodingToolOptions {
    fn default() -> Self {
        Self {
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            mutation_observer: None,
            edit_tool: EditFormat::default(),
            host_files: None,
        }
    }
}

impl CodingToolOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes.max(1);
        self
    }

    /// The edit surface [`CodingToolKind::Edit`] builds.
    pub fn edit_tool(mut self, edit_tool: EditFormat) -> Self {
        self.edit_tool = edit_tool;
        self
    }

    pub fn mutation_observer(
        mut self,
        observer: Arc<dyn crate::WorkspaceMutationObserver>,
    ) -> Self {
        self.mutation_observer = Some(observer);
        self
    }

    /// Text files the host serves in place of local disk. Reads, writes, and
    /// edits of a served path go to the host; see [`crate::HostTextFiles`].
    pub fn host_files(mut self, files: Arc<dyn crate::HostTextFiles>) -> Self {
        self.host_files = Some(files);
        self
    }

    #[cfg(test)]
    pub fn output_budget(&self) -> usize {
        self.max_output_bytes
    }
}

/// A workspace coding tool selected by a host capability set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingToolKind {
    ListDir,
    ReadFile,
    WriteFile,
    Edit,
    Grep,
    Glob,
}

/// Returns one selected SDK coding tool.
pub fn coding_tool(kind: CodingToolKind, options: CodingToolOptions) -> Arc<dyn Tool> {
    match kind {
        CodingToolKind::ListDir => Arc::new(ListDirTool {
            max_output_bytes: options.max_output_bytes,
        }),
        CodingToolKind::ReadFile => Arc::new(ReadFileTool {
            max_output_bytes: options.max_output_bytes,
            host_files: options.host_files.clone(),
        }),
        CodingToolKind::WriteFile => Arc::new(WriteFileTool {
            max_output_bytes: options.max_output_bytes,
            mutation_observer: options.mutation_observer.clone(),
            host_files: options.host_files.clone(),
        }),
        CodingToolKind::Edit => super::build_edit_sdk_tool(
            options.edit_tool,
            options.max_output_bytes,
            options.mutation_observer.clone(),
            options.host_files.clone(),
        ),
        CodingToolKind::Grep => Arc::new(GrepTool::grep(options.max_output_bytes)),
        CodingToolKind::Glob => Arc::new(GlobTool::new(options.max_output_bytes)),
    }
}

/// Returns all SDK coding tools as shared trait objects.
pub fn coding_tools(options: CodingToolOptions) -> Vec<Arc<dyn Tool>> {
    [
        CodingToolKind::ListDir,
        CodingToolKind::ReadFile,
        CodingToolKind::WriteFile,
        CodingToolKind::Edit,
        CodingToolKind::Grep,
        CodingToolKind::Glob,
    ]
    .into_iter()
    .map(|kind| coding_tool(kind, options.clone()))
    .collect()
}
