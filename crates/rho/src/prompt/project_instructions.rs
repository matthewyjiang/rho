//! Cached AGENTS.md context, refreshed only at the new-session boundary.

use std::path::{Path, PathBuf};

use super::{agent_instruction_files, push_context_file, PromptSource, PromptSourceKind};

#[derive(Clone)]
pub(super) struct ProjectInstructions {
    cwd: PathBuf,
    pub(super) retained_offset: usize,
    files: Vec<(PathBuf, String)>,
}

impl ProjectInstructions {
    pub(super) fn new(cwd: &Path, home: Option<&Path>, retained_offset: usize) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            retained_offset,
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
        let index = sources
            .iter()
            .position(|source| source.kind == PromptSourceKind::Skills)
            .unwrap_or(sources.len());
        let instruction_sources = self.files.iter().map(|(path, contents)| {
            let start = text.len();
            push_context_file(text, "agents_instructions", path, contents);
            PromptSource {
                kind: PromptSourceKind::Agents,
                path: Some(path.display().to_string()),
                bytes: text.len() - start,
            }
        });
        drop(sources.splice(index..index, instruction_sources));
    }
}
