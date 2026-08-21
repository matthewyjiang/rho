use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    file_picker::{self, FilePaletteEntry, FilePaletteMatches},
    palette::ActivePalette,
    App,
};

impl App {
    pub(super) fn handle_file_palette_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        let Some(ActivePalette::File(matches)) = self.active_palette() else {
            return Ok(false);
        };

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                if !matches.is_empty() {
                    self.input_ui
                        .set_file_selection(if self.input_ui.file_selection() == 0 {
                            matches.len() - 1
                        } else {
                            self.input_ui.file_selection() - 1
                        });
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if !matches.is_empty() {
                    self.input_ui
                        .set_file_selection((self.input_ui.file_selection() + 1) % matches.len());
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(entry) =
                    selected_palette_entry(&matches, self.input_ui.file_selection())
                {
                    self.apply_file_palette_selection(&entry)?;
                }
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.input_ui.set_file_palette_dismissed(true);
                self.input_ui.set_file_selection(0);
                self.input_ui.clear_paste_burst();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Act on the row the user picked.
    ///
    /// A workspace path and a URI template are both references the message
    /// carries as text, so both are written into the composer. A concrete
    /// resource is content, so it becomes an attachment instead.
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
        if self.insert_file_mention_text(path) {
            self.set_status("file path inserted");
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
        let insertion = if self
            .input_ui
            .text()
            .chars()
            .nth(mention.end)
            .is_some_and(char::is_whitespace)
        {
            format!("@{text}")
        } else {
            format!("@{text} ")
        };
        self.replace_input_range(mention.start, mention.end, &insertion);
        self.input_ui.set_file_palette_dismissed(true);
        self.input_ui.set_file_selection(0);
        true
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
        self.input_ui.set_file_palette_dismissed(true);
        self.input_ui.set_file_selection(0);
    }

    /// Matches for the `@` palette, served from the session cache when fresh.
    ///
    /// Get-or-discover: whichever path asks first — a keystroke or a render
    /// frame — runs discovery once and shares the result. An empty answer also
    /// drops any cache left by a mention that is no longer active.
    pub(super) fn file_match_list(&mut self) -> FilePaletteMatches {
        let Some(mention) =
            file_picker::active_file_mention(self.input_ui.text(), self.input_ui.cursor())
        else {
            self.palette_caches.clear_file();
            return FilePaletteMatches::empty();
        };
        if let Some(matches) = self
            .palette_caches
            .fresh_file(&mention.query, super::palette::PALETTE_CACHE_TTL)
        {
            return matches;
        }
        let discovered = self.discover_file_palette_matches(&mention.query);
        self.palette_caches
            .store_file(mention.query, discovered.clone());
        discovered
    }

    /// Rank both sources for one query. The catalog is an in-memory listing
    /// refreshed at connect, so this stays a local lookup on every keystroke.
    fn discover_file_palette_matches(&mut self, query: &str) -> FilePaletteMatches {
        let resources = if self.mcp_catalog.is_empty() {
            Vec::new()
        } else {
            self.mcp_catalog.resources()
        };
        let cwd = self.info.runtime.cwd.clone();
        file_picker::file_palette_matches(
            file_picker::matching_file_paths_cached(
                &cwd,
                query,
                self.palette_caches.workspace_mut(),
            ),
            &resources,
            query,
        )
    }

    pub(super) fn clamp_file_selection(&mut self) {
        let query = file_picker::active_file_mention(self.input_ui.text(), self.input_ui.cursor())
            .map(|mention| mention.query);
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
