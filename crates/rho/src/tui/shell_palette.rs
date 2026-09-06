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
    inline_shell_config::{resolve_shell, ShellFamily},
    App,
};
use crate::paths::home_dir;

impl App {
    /// Complete the word under the cursor like a shell would: a lone match is
    /// inserted at once, several open the palette, none leaves the composer
    /// alone and says so.
    pub(super) fn open_shell_completion(&mut self) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let shell = resolve_shell(&config.inline_shell);
        if !ShellFamily::for_executable(&shell).supports_path_completion() {
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

    // Deliberately retain every matching entry from this one directory, not
    // the recursive workspace index. This costs memory proportional to the
    // matches but never hides a valid shell argument behind a result cap.
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
            // A literal cwd entry must not acquire home expansion on insert
            // or on the next Tab. Keep intentional ~/ scope prefixes intact.
            let prefix = if scope.display_prefix.is_empty() && name.starts_with('~') {
                "./"
            } else {
                &scope.display_prefix
            };
            Some(format!("{prefix}{name}{suffix}"))
        })
        .collect();
    file_picker::sort_paths_for_display(&mut paths);
    DiscoveredFilePaths::complete(paths)
}

/// Single-quote `path` for the POSIX-style shells accepted above.
pub(super) fn shell_quote(path: &str) -> String {
    // Keep home expansion outside quotes when a later component needs them.
    if let Some(relative) = path.strip_prefix("~/") {
        return format!("~/{}", shell_quote(relative));
    }
    let safe = |ch: char| ch.is_alphanumeric() || "-_./~+:@%,=".contains(ch);
    if !path.is_empty() && !path.starts_with('~') && path.chars().all(safe) {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(test)]
#[path = "shell_palette_tests.rs"]
mod tests;
