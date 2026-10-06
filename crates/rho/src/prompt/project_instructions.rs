//! Cached AGENTS.md context, refreshed when entering a different session.

use std::path::{Path, PathBuf};

use super::{agent_instruction_files, push_context_file, PromptSource, PromptSourceKind};

#[derive(Clone)]
pub(super) struct ProjectInstructions {
    cwd: PathBuf,
    files: Vec<(PathBuf, String)>,
}

impl ProjectInstructions {
    pub(super) fn new(cwd: &Path, home: Option<&Path>) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            files: agent_instruction_files(cwd, home),
        }
    }

    pub(super) fn reload(&mut self, home: Option<&Path>) {
        self.files = agent_instruction_files(&self.cwd, home);
    }

    pub(super) fn append_to(&self, text: &mut String, sources: &mut Vec<PromptSource>) {
        if self.files.is_empty() {
            return;
        }
        let start = text.len();
        text.push_str(
            "\nAdditional instructions from AGENTS.md files follow. More specific files appear later and take precedence:\n",
        );
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
