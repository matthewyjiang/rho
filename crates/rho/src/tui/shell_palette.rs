//! Tab path completion policy for the inline shell composer.
//!
//! Shell mode has no `@` or `/` palette, so `Tab` on the word under the
//! cursor completes it against the workspace index instead. The list, its
//! navigation, and its cache are the ordinary path palette in `file_palette`;
//! this module owns only what differs: when Tab opens it, and how a path is
//! written so the shell reads it as one word.

use super::{file_picker, App};

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
                self.set_status("no matching workspace paths");
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
