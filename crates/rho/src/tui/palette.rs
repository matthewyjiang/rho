//! Which palette, if any, the composer shows.
//!
//! One resolution decides between the `/` command palette and the `@` file
//! palette and produces the winning matches, so visibility checks and painted
//! suggestion lists never compute the same match list twice.

use std::time::{Duration, Instant};

use super::{file_picker::FilePaletteMatches, types::CommandChoice, App, ComposerMode};

/// How long a palette discovery pass stays valid for keystroke and render reuse.
pub(super) const PALETTE_CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct FileMatchCache {
    query: String,
    matches: FilePaletteMatches,
    refreshed_at: Instant,
}

/// Discovered skills reused across command palette queries, so typing a slash
/// command does not re-walk skill directories on every keystroke.
struct SkillMatchCache {
    skills: std::sync::Arc<Vec<crate::skills::Skill>>,
    refreshed_at: Instant,
}

/// Session palette caches. Whichever path asks first — a keystroke or a render
/// frame — runs discovery and shares the result through here.
#[derive(Default)]
pub(super) struct PaletteCaches {
    /// Matches for the active `@` query.
    file: Option<FileMatchCache>,
    /// Discovered skills for `/` palette matching.
    skills: Option<SkillMatchCache>,
}

impl PaletteCaches {
    /// Fresh matches for `query`, or `None` when discovery must run again.
    pub(super) fn fresh_file(&self, query: &str, ttl: Duration) -> Option<FilePaletteMatches> {
        let cache = self.file.as_ref()?;
        (cache.query == query && cache.refreshed_at.elapsed() < ttl).then(|| cache.matches.clone())
    }

    pub(super) fn store_file(&mut self, query: String, matches: FilePaletteMatches) {
        self.file = Some(FileMatchCache {
            query,
            matches,
            refreshed_at: Instant::now(),
        });
    }

    pub(super) fn clear_file(&mut self) {
        self.file = None;
    }

    /// Fresh skills, or `None` when discovery must run again.
    pub(super) fn fresh_skills(
        &self,
        ttl: Duration,
    ) -> Option<std::sync::Arc<Vec<crate::skills::Skill>>> {
        let cache = self.skills.as_ref()?;
        (cache.refreshed_at.elapsed() < ttl).then(|| std::sync::Arc::clone(&cache.skills))
    }

    pub(super) fn store_skills(&mut self, skills: std::sync::Arc<Vec<crate::skills::Skill>>) {
        self.skills = Some(SkillMatchCache {
            skills,
            refreshed_at: Instant::now(),
        });
    }

    #[cfg(test)]
    pub(super) fn expire_file(&mut self) {
        if let Some(cache) = self.file.as_mut() {
            cache.refreshed_at = Instant::now() - PALETTE_CACHE_TTL;
        }
    }
}

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
