//! Painted-line cache for live tool cards.
//!
//! Live cards sit outside the history line cache, so a spinning turn used to
//! re-run syntect for every animation frame. Cache lives on [`ToolEntry`]: card
//! replacement (preview/start/update/finish) drops it automatically. Hits key
//! on layout inputs (width, tool-output budget, image budget, theme
//! generation, expanded). The live elapsed suffix is patched in place when its
//! display width is unchanged; a width change (9.9s → 10.0s) rebuilds.
//!
//! In-place card-body edits must clear [`ToolEntry::render_cache`]. Expand
//! toggles are keyed and do not need an explicit clear.

use ratatui::text::Line;

use super::super::{render::display_width, theme::Theme, LiveCardRenderCache, ToolEntry};
use super::{live_elapsed_label, live_shell_elapsed, tool_entry_lines};

impl ToolEntry {
    /// Cached live-card paint. Rebuilds when layout, theme, or expand state
    /// change; refreshes only the elapsed timeout span otherwise.
    pub(in crate::tui) fn rendered_lines(
        &mut self,
        width: usize,
        max_tool_output_lines: usize,
        max_image_height: u16,
    ) -> &[Line<'static>] {
        let theme_generation = Theme::generation();
        let elapsed_label = live_shell_elapsed(self).map(live_elapsed_label);
        let layout_hit = self.render_cache.as_ref().is_some_and(|cache| {
            cache.width == width
                && cache.max_tool_output_lines == max_tool_output_lines
                && cache.max_image_height == max_image_height
                && cache.theme_generation == theme_generation
                && cache.expanded == self.expanded
        });
        let elapsed_hit = self
            .render_cache
            .as_ref()
            .is_some_and(|cache| cache.elapsed_label.as_deref() == elapsed_label.as_deref());
        if layout_hit && elapsed_hit {
            return &self
                .render_cache
                .as_ref()
                .expect("layout hit implies a cache")
                .lines;
        }
        if layout_hit
            && self
                .render_cache
                .as_mut()
                .is_some_and(|cache| patch_elapsed_label(cache, elapsed_label.as_deref()))
        {
            return &self
                .render_cache
                .as_ref()
                .expect("patched cache remains populated")
                .lines;
        }
        let lines = tool_entry_lines(self, width, max_tool_output_lines, max_image_height);
        let elapsed_spans = match elapsed_label.as_deref() {
            Some(suffix) => find_elapsed_spans(&lines, suffix),
            None => Vec::new(),
        };
        #[cfg(test)]
        let paints = self
            .render_cache
            .as_ref()
            .map(|cache| cache.paints)
            .unwrap_or(0)
            .saturating_add(1);
        self.render_cache = Some(Box::new(LiveCardRenderCache {
            width,
            max_tool_output_lines,
            max_image_height,
            theme_generation,
            expanded: self.expanded,
            elapsed_label,
            elapsed_spans,
            lines,
            #[cfg(test)]
            paints,
        }));
        &self
            .render_cache
            .as_ref()
            .expect("render cache populated above")
            .lines
    }

    #[cfg(test)]
    pub(in crate::tui) fn render_cache_theme_generation(&self) -> Option<u64> {
        self.render_cache
            .as_ref()
            .map(|cache| cache.theme_generation)
    }

    #[cfg(test)]
    pub(in crate::tui) fn render_cache_paints(&self) -> u32 {
        self.render_cache
            .as_ref()
            .map(|cache| cache.paints)
            .unwrap_or(0)
    }
}

fn find_elapsed_spans(lines: &[Line<'static>], suffix: &str) -> Vec<(usize, usize)> {
    lines
        .iter()
        .enumerate()
        .flat_map(|(line_index, line)| {
            line.spans
                .iter()
                .enumerate()
                .filter_map(move |(span_index, span)| {
                    (span.content.as_ref() == suffix).then_some((line_index, span_index))
                })
        })
        .collect()
}

/// Rewrite cached elapsed suffixes when the new label has the same display
/// width. Returns false when wrap would change (caller must rebuild).
fn patch_elapsed_label(cache: &mut LiveCardRenderCache, new_label: Option<&str>) -> bool {
    let (Some(old), Some(new)) = (cache.elapsed_label.as_deref(), new_label) else {
        return false;
    };
    if cache.elapsed_spans.is_empty() || display_width(old) != display_width(new) {
        return false;
    }
    for &(line_index, span_index) in &cache.elapsed_spans {
        let Some(span) = cache
            .lines
            .get_mut(line_index)
            .and_then(|line| line.spans.get_mut(span_index))
        else {
            return false;
        };
        span.content = new.to_string().into();
    }
    cache.elapsed_label = Some(new.to_string());
    true
}

#[cfg(test)]
#[path = "live_card_cache_tests.rs"]
mod tests;
