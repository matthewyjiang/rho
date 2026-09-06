//! Tab path completion policy for the inline shell composer.
//!
//! Shell mode has no `@` or `/` palette, so `Tab` on the word under the
//! cursor completes it the way a shell would: one path component at a time,
//! against the entries of one directory. The list, its navigation, and its
//! cache are the ordinary path palette in `file_palette`; this module owns
//! what differs: when Tab opens it, which candidates it offers, and how a
//! path is written so the shell reads it as one word.

use std::path::Path;

use super::{
    file_picker::{self, DirectoryScope, DiscoveredFilePaths},
    App,
};
use crate::paths::home_dir;

impl App {
    /// Complete the word under the cursor like a shell would: a lone match is
    /// inserted at once, several open the palette, none leaves the composer
    /// alone and says so.
    pub(super) fn open_shell_completion(&mut self) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let shell = if config.inline_shell.trim().is_empty() {
            super::inline_shell::default_shell()
        } else {
            config.inline_shell
        };
        if !supports_path_completion(&shell) {
            self.close_file_palette();
            self.set_status("path completion requires a POSIX-style shell; command left unchanged");
            return Ok(());
        }
        let anchor =
            file_picker::shell_word_at_cursor(self.input_ui.text(), self.input_ui.cursor()).start;
        self.input_ui.set_shell_completion_anchor(Some(anchor));
        self.input_ui.set_file_palette_dismissed(false);
        self.input_ui.set_file_selection(0);
        let matches = self.file_match_list();
        match matches.len() {
            0 => {
                self.set_status("no matching paths");
            }
            1 => {
                if let Some(entry) = matches.get(0) {
                    self.apply_file_palette_selection(&entry)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Entries of the directory `word` names, filtered to those whose name
/// starts with the word's last component, in a shell's order.
///
/// This is a plain directory listing rather than the workspace index because
/// a shell sees the filesystem as it is: `target/`, `.git/`, and untracked
/// files are all valid arguments. Hidden entries still hide until the
/// component starts with `.`, as in bash.
///
/// The trailing `/` on a directory is load-bearing on both sides: it is what
/// the composer shows and inserts, and it is how the write-back knows to
/// leave off the space so the next Tab descends. Do not strip it.
pub(super) fn shell_word_candidates(cwd: &Path, word: &str) -> DiscoveredFilePaths {
    shell_word_candidates_in(cwd, word, home_dir().as_deref())
}

fn shell_word_candidates_in(cwd: &Path, word: &str, home: Option<&Path>) -> DiscoveredFilePaths {
    let (scope, partial) = match file_picker::directory_scope(cwd, word, home) {
        Some((scope, residual)) => (scope, residual),
        // A word with a `/` that names no existing directory has nothing to
        // offer. A bare word lists `cwd`.
        None if word.contains('/') => return DiscoveredFilePaths::complete(Vec::new()),
        None => (
            DirectoryScope {
                root: cwd.to_path_buf(),
                display_prefix: String::new(),
            },
            word.to_string(),
        ),
    };
    let show_hidden = partial.starts_with('.');

    let mut paths: Vec<String> = std::fs::read_dir(&scope.root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(&partial) || (!show_hidden && name.starts_with('.')) {
                return None;
            }
            // Labelling only, not walking: following a symlink here decides
            // whether the next Tab descends, as bash does.
            let suffix = if entry.path().is_dir() { "/" } else { "" };
            Some(format!("{}{name}{suffix}", scope.display_prefix))
        })
        .collect();
    file_picker::sort_paths_for_display(&mut paths);
    DiscoveredFilePaths::complete(paths)
}

/// Completion uses POSIX-style quoting, never PowerShell or cmd syntax.
fn supports_path_completion(shell: &str) -> bool {
    let name = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell);
    matches!(
        name.to_ascii_lowercase().trim_end_matches(".exe"),
        "sh" | "bash" | "zsh" | "dash" | "ksh"
    )
}

/// Single-quote `path` for the POSIX-style shells accepted above.
pub(super) fn shell_quote(path: &str) -> String {
    // Keep home expansion outside quotes when a later component needs them.
    if let Some(relative) = path.strip_prefix("~/") {
        return format!("~/{}", shell_quote(relative));
    }
    let safe = |ch: char| ch.is_alphanumeric() || "-_./~+:@%,=".contains(ch);
    if !path.is_empty() && path.chars().all(safe) {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(test)]
#[path = "shell_palette_tests.rs"]
mod tests;
