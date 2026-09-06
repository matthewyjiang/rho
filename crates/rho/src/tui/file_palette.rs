//! The path palette: `@` mentions in the normal composer, Tab completion in
//! shell mode.
//!
//! Both are the same list over the same workspace index. They differ in how
//! the token under the cursor is found ([`App::active_path_token`]) and how a
//! picked path is written back ([`App::apply_file_palette_selection`]); the
//! navigation, caching, and rendering are shared.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    file_picker::{self, FileMention, FilePaletteEntry, FilePaletteMatches, PathTokenSource},
    palette::ActivePalette,
    shell_palette, App,
};

/// What follows a path written into the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenTerminator {
    /// The path is a finished argument; separate it from what comes next.
    Space,
    /// The path is a directory the user will keep descending into.
    None,
}

impl App {
    pub(super) fn handle_file_palette_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        let Some(ActivePalette::File(matches)) = self.active_palette() else {
            // Shell mode has no auto-open: Tab on a bare word opens completion.
            if self.input_ui.shell_mode().is_some()
                && (key.modifiers, key.code) == (KeyModifiers::NONE, KeyCode::Tab)
            {
                self.open_shell_completion()?;
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                return Ok(true);
            }
            return Ok(false);
        };

        let handled = match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                let selection = self.input_ui.file_selection();
                self.input_ui.set_file_selection(if selection == 0 {
                    matches.len() - 1
                } else {
                    selection - 1
                });
                true
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                self.input_ui
                    .set_file_selection((self.input_ui.file_selection() + 1) % matches.len());
                true
            }
            (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(entry) =
                    selected_palette_entry(&matches, self.input_ui.file_selection())
                {
                    self.apply_file_palette_selection(&entry)?;
                }
                true
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.close_file_palette();
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

    /// Close the palette until the next typed edit (`@`) or the next Tab
    /// (shell mode).
    pub(super) fn close_file_palette(&mut self) {
        self.input_ui.set_file_palette_dismissed(true);
        self.input_ui.set_shell_completion_anchor(None);
        self.input_ui.set_file_selection(0);
    }

    /// Act on the row the user picked.
    ///
    /// A workspace path and a URI template are both references the message
    /// carries as text, so both are written into the composer. A concrete
    /// resource is content, so it becomes an attachment instead. A shell word
    /// is written as a path the shell can read.
    pub(super) fn apply_file_palette_selection(
        &mut self,
        entry: &FilePaletteEntry,
    ) -> anyhow::Result<()> {
        match entry {
            FilePaletteEntry::WorkspaceFile(path) => self.insert_selected_file_path(path),
            // A template URI carries RFC 6570 placeholders, so there is nothing
            // to read until a person fills them in. It goes in as text.
            FilePaletteEntry::McpResource(resource) if resource.templated => {
                if self.insert_file_mention_text(&resource.uri) {
                    self.set_status("resource template inserted; fill in the placeholders");
                }
            }
            FilePaletteEntry::McpResource(resource) => self.start_mcp_resource_attach(resource)?,
        }
        Ok(())
    }

    pub(super) fn insert_selected_file_path(&mut self, path: &str) {
        let Some(token) = self.active_path_token() else {
            return;
        };
        match token.source {
            PathTokenSource::Mention => {
                if self.insert_file_mention_text(path) {
                    self.set_status("file path inserted");
                }
            }
            // A directory is one component of a longer path: no space after
            // it, so the next Tab keeps descending from where this one left off.
            PathTokenSource::ShellWord => self.replace_path_token(
                &token,
                shell_palette::shell_quote(path),
                if path.ends_with('/') {
                    TokenTerminator::None
                } else {
                    TokenTerminator::Space
                },
            ),
        }
    }

    /// Replace the active `@` token with `@{text}` and close the palette.
    ///
    /// Returns false when the mention is already gone, so callers do not report
    /// an insertion that did not happen.
    fn insert_file_mention_text(&mut self, text: &str) -> bool {
        let Some(mention) =
            file_picker::active_file_mention(self.input_ui.text(), self.input_ui.cursor())
        else {
            return false;
        };
        self.replace_path_token(&mention, format!("@{text}"), TokenTerminator::Space);
        true
    }

    /// Write `insertion` over the token and close the palette. A `Space`
    /// terminator is added only when the token is not already followed by one.
    fn replace_path_token(
        &mut self,
        token: &FileMention,
        mut insertion: String,
        terminator: TokenTerminator,
    ) {
        let next_is_space = self
            .input_ui
            .text()
            .chars()
            .nth(token.end)
            .is_some_and(char::is_whitespace);
        if terminator == TokenTerminator::Space && !next_is_space {
            insertion.push(' ');
        }
        self.replace_input_range(token.start, token.end, &insertion);
        self.close_file_palette();
    }

    /// Remove the active `@` token entirely, for a selection whose content is
    /// attached rather than referenced by name.
    pub(super) fn clear_active_file_mention(&mut self) {
        let Some(mention) =
            file_picker::active_file_mention(self.input_ui.text(), self.input_ui.cursor())
        else {
            return;
        };
        self.replace_input_range(mention.start, mention.end, "");
        self.close_file_palette();
    }

    /// The token the path palette is matching on right now, if any.
    ///
    /// Outside shell mode that is an `@` mention. In shell mode it is the bare
    /// word Tab opened completion on, for as long as the cursor stays in it.
    pub(super) fn active_path_token(&self) -> Option<FileMention> {
        let (text, cursor) = (self.input_ui.text(), self.input_ui.cursor());
        if self.input_ui.shell_mode().is_some() {
            let anchor = self.input_ui.shell_completion_anchor()?;
            file_picker::anchored_shell_word(text, cursor, anchor)
        } else {
            file_picker::active_file_mention(text, cursor)
        }
    }

    /// Matches for the path palette, served from the session cache when fresh.
    ///
    /// Get-or-discover: whichever path asks first — a keystroke or a render
    /// frame — runs discovery once and shares the result. An empty answer also
    /// drops any cache left by a token that is no longer active.
    pub(super) fn file_match_list(&mut self) -> FilePaletteMatches {
        let Some(token) = self.active_path_token() else {
            self.palette_caches.clear_file();
            return FilePaletteMatches::empty();
        };
        if let Some(matches) = self.palette_caches.fresh_file(
            token.source,
            &token.query,
            super::palette::PALETTE_CACHE_TTL,
        ) {
            return matches;
        }
        let discovered = self.discover_file_palette_matches(&token);
        self.palette_caches
            .store_file(token.source, token.query, discovered.clone());
        discovered
    }

    /// Candidates for one token. A mention fuzzy-searches the whole workspace
    /// index plus the MCP catalog (an in-memory listing refreshed at connect,
    /// so this stays a local lookup on every keystroke). A shell word lists one
    /// directory, one component at a time, as a shell does.
    fn discover_file_palette_matches(&mut self, token: &FileMention) -> FilePaletteMatches {
        let cwd = self.info.runtime.cwd.clone();
        match token.source {
            PathTokenSource::Mention => {
                let discovered = file_picker::matching_file_paths_cached(
                    &cwd,
                    &token.query,
                    self.palette_caches.workspace_mut(),
                );
                let resources = if self.mcp_catalog.is_empty() {
                    Vec::new()
                } else {
                    self.mcp_catalog.resources()
                };
                file_picker::file_palette_matches(discovered, &resources, &token.query)
            }
            PathTokenSource::ShellWord => FilePaletteMatches::shell_words(
                shell_palette::shell_word_candidates(&cwd, &token.query),
            ),
        }
    }

    /// Reset the highlight when the token changes and keep it inside the list.
    /// Keep a shell anchor through zero matches so correcting the word restores
    /// the list, but drop it when the cursor leaves that word.
    pub(super) fn clamp_file_selection(&mut self) {
        let query = self.active_path_token().map(|token| token.query);
        if query.is_none() {
            self.input_ui.set_shell_completion_anchor(None);
        }
        if self.input_ui.file_query() != query.as_deref() {
            self.input_ui.set_file_query(query);
            self.input_ui.set_file_selection(0);
        }

        let match_count = self.file_match_list().len();
        if match_count == 0 {
            self.input_ui.set_file_selection(0);
        } else if self.input_ui.file_selection() >= match_count {
            self.input_ui.set_file_selection(match_count - 1);
        }
    }
}

fn selected_palette_entry(
    matches: &FilePaletteMatches,
    selection: usize,
) -> Option<FilePaletteEntry> {
    matches.get(selection.min(matches.len().saturating_sub(1)))
}

#[cfg(test)]
#[path = "file_palette_tests.rs"]
mod tests;
