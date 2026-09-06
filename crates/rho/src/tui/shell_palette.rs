//! Tab path completion for the inline shell composer.
//!
//! Shell mode has no `@` or `/` palette, so `Tab` on the word under the cursor
//! opens a workspace path list instead. The list is the same index the `@`
//! palette uses, so ignore rules and `dir/residual` scoping match. A single
//! match inserts at once, like a shell; several open the palette.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    file_picker::{self, DiscoveredFilePaths},
    palette::ActivePalette,
    App,
};

/// Open completion state for the shell composer.
///
/// `anchor` is the char offset where the completed word starts. The palette
/// follows edits inside that word and closes once the cursor leaves it.
#[derive(Clone, Debug)]
pub(super) struct ShellCompletion {
    anchor: usize,
    query: String,
    matches: DiscoveredFilePaths,
    selection: usize,
}

impl ShellCompletion {
    pub(super) fn matches(&self) -> &DiscoveredFilePaths {
        &self.matches
    }

    pub(super) fn selection(&self) -> usize {
        self.selection
    }
}

impl App {
    /// Shell-mode `Tab`, palette navigation, and accept/dismiss keys.
    ///
    /// Returns true when the key was consumed. Runs before the generic Esc
    /// handling so closing the palette does not also leave shell mode.
    pub(super) fn handle_shell_palette_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if self.input_ui.shell_mode().is_none() {
            return Ok(false);
        }
        let Some(ActivePalette::ShellPath(matches)) = self.active_palette() else {
            if key.modifiers == KeyModifiers::NONE && key.code == KeyCode::Tab {
                self.open_shell_completion();
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                return Ok(true);
            }
            return Ok(false);
        };

        // The clamp closes an emptied list on every edit; guard the modulo
        // arithmetic below anyway rather than trust that ordering forever.
        let count = matches.as_slice().len();
        if count == 0 {
            self.input_ui.set_shell_completion(None);
            return Ok(false);
        }
        let handled = match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                self.input_ui.update_shell_completion(|state| {
                    state.selection = (state.selection + count - 1) % count
                });
                true
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                self.input_ui.update_shell_completion(|state| {
                    state.selection = (state.selection + 1) % count
                });
                true
            }
            (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::NONE, KeyCode::Enter) => {
                let selection = self
                    .input_ui
                    .shell_completion()
                    .map_or(0, ShellCompletion::selection)
                    .min(count - 1);
                let path = matches.as_slice()[selection].clone();
                self.input_ui.set_shell_completion(None);
                self.insert_shell_completion(&path);
                true
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.input_ui.set_shell_completion(None);
                true
            }
            _ => false,
        };
        if handled {
            self.input_ui.clear_paste_burst();
            self.ctrl_c_streak = 0;
        }
        Ok(handled)
    }

    /// Complete the word under the cursor: insert a lone match, open the
    /// palette for several, and say so when nothing matches.
    fn open_shell_completion(&mut self) {
        let word = file_picker::word_at_cursor(self.input_ui.text(), self.input_ui.cursor());
        let (anchor, query) = (word.start, word.head.to_string());
        let matches = self.discover_shell_completion_matches(&query);
        match matches.as_slice() {
            [] => self.set_status("no matching workspace paths"),
            [only] => {
                let only = only.clone();
                self.insert_shell_completion(&only);
            }
            _ => self.input_ui.set_shell_completion(Some(ShellCompletion {
                anchor,
                query,
                matches,
                selection: 0,
            })),
        }
    }

    /// Replace the word under the cursor with `path`, quoted when the shell
    /// would otherwise split or expand it.
    fn insert_shell_completion(&mut self, path: &str) {
        let word = file_picker::word_at_cursor(self.input_ui.text(), self.input_ui.cursor());
        let next_is_space = self
            .input_ui
            .text()
            .chars()
            .nth(word.end)
            .is_some_and(char::is_whitespace);
        let mut insertion = shell_quote(path);
        if !next_is_space {
            insertion.push(' ');
        }
        self.replace_input_range(word.start, word.end, &insertion);
    }

    fn discover_shell_completion_matches(&mut self, query: &str) -> DiscoveredFilePaths {
        let cwd = self.info.runtime.cwd.clone();
        file_picker::matching_file_paths_cached(&cwd, query, self.palette_caches.workspace_mut())
    }

    /// Keep the open palette in step with the composer: refilter while the
    /// cursor stays inside the anchored word, close once it leaves or nothing
    /// matches any more.
    pub(super) fn clamp_shell_completion(&mut self) {
        let Some((anchor, query)) = self
            .input_ui
            .shell_completion()
            .map(|state| (state.anchor, state.query.clone()))
        else {
            return;
        };
        if self.input_ui.shell_mode().is_none() {
            self.input_ui.set_shell_completion(None);
            return;
        }
        let word = file_picker::word_at_cursor(self.input_ui.text(), self.input_ui.cursor());
        if word.start != anchor {
            self.input_ui.set_shell_completion(None);
            return;
        }
        if word.head != query {
            let query = word.head.to_string();
            let matches = self.discover_shell_completion_matches(&query);
            if matches.as_slice().is_empty() {
                self.input_ui.set_shell_completion(None);
                return;
            }
            self.input_ui.update_shell_completion(|state| {
                state.query = query;
                state.matches = matches;
                state.selection = 0;
            });
        }
        self.input_ui.update_shell_completion(|state| {
            state.selection = state
                .selection
                .min(state.matches.as_slice().len().saturating_sub(1));
        });
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
