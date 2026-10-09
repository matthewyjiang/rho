//! Cached AGENTS.md context, refreshed when entering a different session.

use std::path::{Path, PathBuf};

use super::{
    agent_instruction_files, agent_instruction_paths, push_context_file, read_existing_files,
    PromptSource, PromptSourceKind,
};

#[derive(Clone)]
pub(super) struct ProjectInstructions {
    cwd: PathBuf,
    /// Added workspace directories whose AGENTS.md chains also load.
    added_dirs: Vec<PathBuf>,
    files: Vec<(PathBuf, String)>,
}

impl ProjectInstructions {
    pub(super) fn new(cwd: &Path, home: Option<&Path>) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            added_dirs: Vec::new(),
            files: agent_instruction_files(cwd, &[], home),
        }
    }

    pub(super) fn reload(&mut self, home: Option<&Path>) {
        self.files = agent_instruction_files(&self.cwd, &self.added_dirs, home);
    }

    /// Replaces the added directories. Files already loaded keep their cached
    /// contents; only new candidates are read. Returns the newly loaded files,
    /// in prompt order.
    pub(super) fn set_added_dirs(
        &mut self,
        added_dirs: &[PathBuf],
        home: Option<&Path>,
    ) -> Vec<(PathBuf, String)> {
        if self.added_dirs == added_dirs {
            return Vec::new();
        }
        self.added_dirs = added_dirs.to_vec();
        let previous = std::mem::take(&mut self.files);
        let mut loaded = Vec::new();
        for path in agent_instruction_paths(&self.cwd, &self.added_dirs, home) {
            match previous.iter().find(|(cached, _)| *cached == path) {
                Some(file) => self.files.push(file.clone()),
                None => {
                    let read = read_existing_files(vec![path]);
                    loaded.extend(read.iter().cloned());
                    self.files.extend(read);
                }
            }
        }
        loaded
    }

    pub(super) fn append_to(&self, text: &mut String, sources: &mut Vec<PromptSource>) {
        if !self.added_dirs.is_empty() {
            let start = text.len();
            text.push_str("\nAdditional workspace directories added for this session:\n");
            for dir in &self.added_dirs {
                text.push_str("- ");
                text.push_str(&crate::paths::prompt_data(dir));
                text.push('\n');
            }
            sources[0].bytes += text.len() - start;
        }
        if self.files.is_empty() {
            return;
        }
        let start = text.len();
        // Separate chains are concatenated once directories are added, so
        // "later wins" would let one tree's root override another's leaf.
        text.push_str(if self.added_dirs.is_empty() {
            "\nAdditional instructions from AGENTS.md files follow. More specific files appear later and take precedence:\n"
        } else {
            "\nAdditional instructions from AGENTS.md files follow. Each file applies to its own directory and everything below it. Where files conflict, the file in the deeper directory takes precedence; files in unrelated directories do not override each other:\n"
        });
        sources[0].bytes += text.len() - start;
        for (path, contents) in &self.files {
            let start = text.len();
            push_context_file(text, "agents_instructions", path, contents);
            sources.push(PromptSource {
                kind: PromptSourceKind::Agents,
                path: Some(path.display().to_string()),
                bytes: text.len() - start,
            });
        }
    }
}

/// Model context and transcript notice for a directory added mid-session.
///
/// The live system prompt stays fixed for prompt-cache stability, so the
/// directory and any AGENTS.md files it brings arrive as appended context.
pub(crate) fn added_dir_context(dir: &Path, files: &[(PathBuf, String)]) -> (String, String) {
    let mut model = format!(
        "[workspace directory added]\n\nThe user added {} to this session's workspace scope. Reads there follow the same permission rules as the working directory.\n",
        crate::paths::prompt_data(dir),
    );
    if !files.is_empty() {
        model.push_str(
            "\nAdditional instructions from AGENTS.md files follow. They apply to work under that directory:\n",
        );
        for (path, contents) in files {
            push_context_file(&mut model, "agents_instructions", path, contents);
        }
    }
    let display = format!("added directory {}", crate::paths::display(dir));
    (model, display)
}
