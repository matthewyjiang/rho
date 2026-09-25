//! Which palette, if any, the composer shows.
//!
//! One resolution decides between the `/` command palette and the path
//! palette (`@` mentions, or Tab completion in shell mode) and produces the
//! winning matches, so visibility checks and painted suggestion lists never
//! compute the same match list twice.

use std::time::{Duration, Instant};

use ratatui::text::Line;

use super::{
    composer_pointer::ComposerHit,
    file_picker::{FilePaletteMatches, PathTokenSource, WorkspacePathCache},
    types::CommandChoice,
    App, ComposerMode,
};

/// How long a palette discovery pass stays valid for keystroke and render reuse.
pub(super) const PALETTE_CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct FileMatchCache {
    source: PathTokenSource,
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
    /// Workspace walk shared by different `@` queries against the same root.
    workspace: WorkspacePathCache,
}

impl PaletteCaches {
    /// Fresh matches for `query` from `source`, or `None` when discovery must
    /// run again. The source is part of the key: a `@src/` list carries MCP
    /// resources a shell word must never be offered.
    pub(super) fn fresh_file(
        &self,
        source: PathTokenSource,
        query: &str,
        ttl: Duration,
    ) -> Option<FilePaletteMatches> {
        let cache = self.file.as_ref()?;
        (cache.source == source && cache.query == query && cache.refreshed_at.elapsed() < ttl)
            .then(|| cache.matches.clone())
    }

    pub(super) fn store_file(
        &mut self,
        source: PathTokenSource,
        query: String,
        matches: FilePaletteMatches,
    ) {
        self.file = Some(FileMatchCache {
            source,
            query,
            matches,
            refreshed_at: Instant::now(),
        });
    }

    pub(super) fn clear_file(&mut self) {
        self.file = None;
    }

    pub(super) fn workspace_mut(&mut self) -> &mut WorkspacePathCache {
        &mut self.workspace
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

    #[cfg(test)]
    pub(super) fn expire_workspace(&mut self) {
        self.workspace.expire();
    }
}

/// The palette the composer currently shows, with its matches.
///
/// The command palette wins when both could answer. In shell mode only the
/// path palette can show, and only once Tab has opened it.
#[derive(Debug)]
pub(super) enum ActivePalette {
    Command(Vec<CommandChoice>),
    File(FilePaletteMatches),
}

/// A painted palette row, by absolute index into the active palette's
/// matches rather than its offset in the scrolled window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaletteRow {
    Command(usize),
    File(usize),
}

/// Palette lines painted above the composer, and which of them are rows a
/// pointer can pick. Hit line indices count from the first palette line; the
/// highlighted row's hit is marked active.
#[derive(Debug, Default)]
pub(super) struct PaletteFrame {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) hits: Vec<ComposerHit<PaletteRow>>,
}

impl PaletteFrame {
    /// Append a pickable row; `selected` marks the highlighted one.
    pub(super) fn push_row(&mut self, line: Line<'static>, row: PaletteRow, selected: bool) {
        let index = self.lines.len();
        let hit = ComposerHit::rows(index..index + 1, row);
        self.hits.push(hit.with_active(selected));
        self.lines.push(line);
    }

    /// Append a line that is not a row, such as a scroll footer.
    pub(super) fn push_line(&mut self, line: Line<'static>) {
        self.lines.push(line);
    }
}

impl App {
    /// Resolve the active palette, computing its matches at most once per ask.
    pub(super) fn active_palette(&mut self) -> Option<ActivePalette> {
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return None;
        }
        if self.input_ui.shell_mode().is_none() {
            if let Some(matches) = self.visible_command_matches() {
                return Some(ActivePalette::Command(matches));
            }
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
