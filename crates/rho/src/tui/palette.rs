//! Which palette, if any, the composer shows.
//!
//! One resolution decides between the `/` command palette and the `@` file
//! palette and produces the winning matches, so visibility checks and painted
//! suggestion lists never compute the same match list twice.

use super::{file_picker::FilePaletteMatches, types::CommandChoice, App, ComposerMode};

/// The palette the composer currently shows, with its matches.
///
/// The command palette wins when both could answer.
#[derive(Debug)]
pub(super) enum ActivePalette {
    Command(Vec<CommandChoice>),
    File(FilePaletteMatches),
}

impl App {
    /// Resolve the active palette, computing its matches at most once per ask.
    pub(super) fn active_palette(&mut self) -> Option<ActivePalette> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input)
            || self.input_ui.shell_mode().is_some()
        {
            return None;
        }
        if let Some(matches) = self.visible_command_matches() {
            return Some(ActivePalette::Command(matches));
        }
        if self.input_ui.file_palette_dismissed() {
            return None;
        }
        let matches = self.file_match_list();
        (!matches.is_empty()).then_some(ActivePalette::File(matches))
    }

    /// Test-facing predicate; production code resolves [`App::active_palette`]
    /// so the matches are computed once.
    #[cfg(test)]
    pub(super) fn command_palette_visible(&mut self) -> bool {
        matches!(self.active_palette(), Some(ActivePalette::Command(_)))
    }
}
