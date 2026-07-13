use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{file_picker, App, ComposerMode, FileMatchCache};

impl App {
    pub(super) fn handle_file_palette_key(&mut self, key: KeyEvent) -> anyhow::Result<bool> {
        if !self.file_palette_visible() {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Up) => {
                let matches = self.file_matches();
                if !matches.is_empty() {
                    self.file_selection = if self.file_selection == 0 {
                        matches.len() - 1
                    } else {
                        self.file_selection - 1
                    };
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.file_matches();
                if !matches.is_empty() {
                    self.file_selection = (self.file_selection + 1) % matches.len();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(path) = self.selected_file_path() {
                    self.insert_selected_file_path(&path);
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.file_palette_dismissed = true;
                self.file_selection = 0;
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn insert_selected_file_path(&mut self, path: &str) {
        let Some(mention) = file_picker::active_file_mention(&self.input, self.input_cursor) else {
            return;
        };
        let insertion = format!("@{path} ");
        self.replace_input_range(mention.start, mention.end, &insertion);
        self.file_palette_dismissed = true;
        self.file_selection = 0;
        self.status = "file path inserted".into();
    }

    pub(super) fn file_matches(&self) -> Arc<Vec<String>> {
        let Some(mention) = file_picker::active_file_mention(&self.input, self.input_cursor) else {
            return Arc::new(Vec::new());
        };
        if let Some(cache) = &self.file_match_cache {
            if cache.query == mention.query {
                return std::sync::Arc::clone(&cache.matches);
            }
        }
        file_picker::matching_file_paths(&self.info.cwd, &mention.query)
    }

    fn refresh_file_match_cache(&mut self) {
        let Some(mention) = file_picker::active_file_mention(&self.input, self.input_cursor) else {
            self.file_match_cache = None;
            return;
        };
        if self
            .file_match_cache
            .as_ref()
            .is_some_and(|cache| cache.query == mention.query)
        {
            return;
        }
        self.file_match_cache = Some(FileMatchCache {
            query: mention.query.clone(),
            matches: file_picker::matching_file_paths(&self.info.cwd, &mention.query),
        });
    }

    pub(super) fn selected_file_path(&self) -> Option<String> {
        let matches = self.file_matches();
        matches
            .get(self.file_selection.min(matches.len().saturating_sub(1)))
            .cloned()
    }

    pub(super) fn file_palette_visible(&self) -> bool {
        matches!(self.composer, ComposerMode::Input)
            && !self.file_palette_dismissed
            && !self.command_palette_visible()
            && file_picker::active_file_mention(&self.input, self.input_cursor).is_some()
            && !self.file_matches().is_empty()
    }

    pub(super) fn clamp_file_selection(&mut self) {
        let query = file_picker::active_file_mention(&self.input, self.input_cursor)
            .map(|mention| mention.query);
        if self.file_query != query {
            self.file_query = query;
            self.file_selection = 0;
        }
        self.refresh_file_match_cache();

        let match_count = self.file_matches().len();
        if match_count == 0 {
            self.file_selection = 0;
        } else if self.file_selection >= match_count {
            self.file_selection = match_count - 1;
        }
    }

    pub(super) fn handle_running_file_palette_key(
        &mut self,
        key: KeyEvent,
    ) -> anyhow::Result<bool> {
        self.handle_file_palette_key(key)
    }
}
