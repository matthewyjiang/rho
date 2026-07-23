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
                    self.input_ui.file_selection = if self.input_ui.file_selection == 0 {
                        matches.len() - 1
                    } else {
                        self.input_ui.file_selection - 1
                    };
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                let matches = self.file_matches();
                if !matches.is_empty() {
                    self.input_ui.file_selection =
                        (self.input_ui.file_selection + 1) % matches.len();
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Tab) | (KeyModifiers::NONE, KeyCode::Enter) => {
                if let Some(path) = self.selected_file_path() {
                    self.insert_selected_file_path(&path);
                }
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Esc) => {
                self.input_ui.file_palette_dismissed = true;
                self.input_ui.file_selection = 0;
                self.input_ui.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn insert_selected_file_path(&mut self, path: &str) {
        let Some(mention) =
            file_picker::active_file_mention(&self.input_ui.input, self.input_ui.input_cursor)
        else {
            return;
        };
        let insertion = if self
            .input_ui
            .input
            .chars()
            .nth(mention.end)
            .is_some_and(char::is_whitespace)
        {
            format!("@{path}")
        } else {
            format!("@{path} ")
        };
        self.replace_input_range(mention.start, mention.end, &insertion);
        self.input_ui.file_palette_dismissed = true;
        self.input_ui.file_selection = 0;
        self.status = "file path inserted".into();
    }

    pub(super) fn file_matches(&self) -> Arc<Vec<String>> {
        let Some(mention) =
            file_picker::active_file_mention(&self.input_ui.input, self.input_ui.input_cursor)
        else {
            return Arc::new(Vec::new());
        };
        if let Some(cache) = &self.input_ui.file_match_cache {
            if cache.query == mention.query
                && cache.refreshed_at.elapsed() < file_picker::FILE_PATH_CACHE_TTL
            {
                return std::sync::Arc::clone(&cache.matches);
            }
        }
        file_picker::matching_file_paths(&self.info.runtime.cwd, &mention.query)
    }

    fn refresh_file_match_cache(&mut self) {
        let Some(mention) =
            file_picker::active_file_mention(&self.input_ui.input, self.input_ui.input_cursor)
        else {
            self.input_ui.file_match_cache = None;
            return;
        };
        if self
            .input_ui
            .file_match_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.query == mention.query
                    && cache.refreshed_at.elapsed() < file_picker::FILE_PATH_CACHE_TTL
            })
        {
            return;
        }
        self.input_ui.file_match_cache = Some(FileMatchCache {
            query: mention.query.clone(),
            matches: file_picker::matching_file_paths(&self.info.runtime.cwd, &mention.query),
            refreshed_at: std::time::Instant::now(),
        });
    }

    pub(super) fn selected_file_path(&self) -> Option<String> {
        let matches = self.file_matches();
        matches
            .get(
                self.input_ui
                    .file_selection
                    .min(matches.len().saturating_sub(1)),
            )
            .cloned()
    }

    pub(super) fn file_palette_visible(&self) -> bool {
        matches!(self.input_ui.composer, ComposerMode::Input)
            && !self.input_ui.file_palette_dismissed
            && !self.command_palette_visible()
            && file_picker::active_file_mention(&self.input_ui.input, self.input_ui.input_cursor)
                .is_some()
            && !self.file_matches().is_empty()
    }

    pub(super) fn clamp_file_selection(&mut self) {
        let query =
            file_picker::active_file_mention(&self.input_ui.input, self.input_ui.input_cursor)
                .map(|mention| mention.query);
        if self.input_ui.file_query != query {
            self.input_ui.file_query = query;
            self.input_ui.file_selection = 0;
        }
        self.refresh_file_match_cache();

        let match_count = self.file_matches().len();
        if match_count == 0 {
            self.input_ui.file_selection = 0;
        } else if self.input_ui.file_selection >= match_count {
            self.input_ui.file_selection = match_count - 1;
        }
    }

    pub(super) fn handle_running_file_palette_key(
        &mut self,
        key: KeyEvent,
    ) -> anyhow::Result<bool> {
        self.handle_file_palette_key(key)
    }
}
