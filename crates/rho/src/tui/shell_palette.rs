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
    file_picker::{self, DiscoveredFilePaths},
    App,
};
use crate::paths::home_dir;

impl App {
    /// Complete the word under the cursor like a shell would: a lone match is
    /// inserted at once, several open the palette, none leaves the composer
    /// alone and says so.
    pub(super) fn open_shell_completion(&mut self) {
        let anchor =
            file_picker::word_at_cursor(self.input_ui.text(), self.input_ui.cursor()).start;
        self.input_ui.set_shell_completion_anchor(Some(anchor));
        self.input_ui.set_file_palette_dismissed(false);
        self.input_ui.set_file_selection(0);
        let matches = self.file_match_list();
        match matches.len() {
            0 => {
                self.input_ui.set_shell_completion_anchor(None);
                self.set_status("no matching paths");
            }
            1 => {
                if let Some(entry) = matches.get(0) {
                    let _ = self.apply_file_palette_selection(&entry);
                }
            }
            _ => {}
        }
    }
}

/// Entries of the directory `word` names, filtered to those whose name
/// starts with the word's last component, in a shell's order: directories
/// end in `/` so the next Tab descends, files do not.
///
/// This is a plain directory listing rather than the workspace index because
/// a shell sees the filesystem as it is: `target/`, `.git/`, and untracked
/// files are all valid arguments. Hidden entries still hide until the
/// component starts with `.`, as in bash.
pub(super) fn shell_word_candidates(cwd: &Path, word: &str) -> DiscoveredFilePaths {
    shell_word_candidates_in(cwd, word, home_dir().as_deref())
}

fn shell_word_candidates_in(cwd: &Path, word: &str, home: Option<&Path>) -> DiscoveredFilePaths {
    let (directory, partial) = word.rsplit_once('/').unwrap_or(("", word));
    let root = match directory {
        "" => cwd.to_path_buf(),
        "/" => Path::new("/").to_path_buf(),
        _ => file_picker::resolve_user_path(cwd, directory, home),
    };
    let show_hidden = partial.starts_with('.');
    let display_prefix = match directory {
        "" => String::new(),
        "/" => "/".into(),
        _ => format!("{directory}/"),
    };

    let mut paths: Vec<String> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !name.starts_with(partial) || (!show_hidden && name.starts_with('.')) {
                return None;
            }
            let is_dir = entry.file_type().ok()?.is_dir();
            Some(if is_dir {
                format!("{display_prefix}{name}/")
            } else {
                format!("{display_prefix}{name}")
            })
        })
        .collect();
    paths.sort_by(|left, right| {
        left.to_ascii_lowercase()
            .cmp(&right.to_ascii_lowercase())
            .then_with(|| left.cmp(right))
    });
    DiscoveredFilePaths::complete(paths)
}

/// Single-quote `path` unless every char is safe in every supported shell.
pub(super) fn shell_quote(path: &str) -> String {
    let safe = |ch: char| ch.is_alphanumeric() || "-_./~+:@%,=".contains(ch);
    if !path.is_empty() && path.chars().all(safe) {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(test)]
#[path = "shell_palette_tests.rs"]
mod tests;
