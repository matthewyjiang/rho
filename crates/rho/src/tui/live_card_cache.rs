//! Painted-line cache for live tool cards.
//!
//! Live cards sit outside the history line cache, so a spinning turn used to
//! re-run syntect for every animation frame. Cache lives on [`ToolEntry`]: card
//! replacement (preview/start/update/finish) drops it automatically. Hits key
//! on layout inputs (width, tool-output budget, image budget, theme
//! generation, syntax readiness, expanded). The live elapsed clock reuses the
//! highlighted header and rebuilds only timeout facts.

use ratatui::text::Line;

use super::super::{
    feed_image::reserve_optional_image_rows,
    render::{pad_display_line, padded_content_width, styled_blank_line},
    theme::Theme,
    LiveCardRenderCache, ToolEntry,
};
use super::{live_elapsed_label, live_shell_elapsed, paint_card_sections, paint_live_prefix};

impl ToolEntry {
    /// Cached live-card paint. Rebuilds when layout, theme, or expand state
    /// change; refreshes only the elapsed prefix otherwise.
    pub(in crate::tui) fn rendered_lines(
        &mut self,
        width: usize,
        max_tool_output_lines: usize,
        max_image_height: u16,
    ) -> &[Line<'static>] {
        let theme_generation = Theme::generation();
        let syntax_ready = super::super::syntax::syntax_set_ready();
        let elapsed_label = live_shell_elapsed(self).map(live_elapsed_label);
        let layout_hit = self.render_cache.as_ref().is_some_and(|cache| {
            cache.width == width
                && cache.max_tool_output_lines == max_tool_output_lines
                && cache.max_image_height == max_image_height
                && cache.theme_generation == theme_generation
                && cache.syntax_ready == syntax_ready
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
            && self.patch_elapsed_prefix(width, max_image_height, elapsed_label.as_deref())
        {
            return &self
                .render_cache
                .as_ref()
                .expect("patched cache remains populated")
                .lines;
        }

        let inner_width = padded_content_width(width);
        let sections = paint_card_sections(
            &self.card,
            inner_width,
            max_tool_output_lines,
            self.expanded,
            live_shell_elapsed(self),
        );
        let last_fact_is_end = sections.last_fact_is_end;
        let prefix_len = sections.prefix.len();
        let header = sections.prefix[..sections.header_len].to_vec();
        let body = sections.body;
        let lines = finish_live_card(self, width, max_image_height, sections.prefix, &body);
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
            syntax_ready,
            expanded: self.expanded,
            elapsed_label,
            last_fact_is_end,
            prefix_len,
            header,
            body,
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

    fn patch_elapsed_prefix(
        &mut self,
        width: usize,
        max_image_height: u16,
        elapsed_label: Option<&str>,
    ) -> bool {
        let Some(cache) = self.render_cache.as_ref() else {
            return false;
        };
        let inner_width = padded_content_width(width);
        let prefix = paint_live_prefix(
            &self.card,
            inner_width,
            live_shell_elapsed(self),
            cache.last_fact_is_end,
            &cache.header,
        );
        if prefix.len() != cache.prefix_len {
            return false;
        }
        let lines = finish_live_card(self, width, max_image_height, prefix, &cache.body);
        let Some(cache) = self.render_cache.as_mut() else {
            return false;
        };
        cache.elapsed_label = elapsed_label.map(str::to_string);
        cache.lines = lines;
        true
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

fn finish_live_card(
    tool: &ToolEntry,
    width: usize,
    max_image_height: u16,
    prefix: Vec<Line<'static>>,
    body: &[Line<'static>],
) -> Vec<Line<'static>> {
    let mut card_lines = prefix;
    card_lines.extend(body.iter().cloned());
    reserve_optional_image_rows(
        &mut card_lines,
        tool.image.as_ref(),
        width,
        max_image_height,
    );
    let padding_style = Theme::tool_card_padding();
    let mut padded = Vec::with_capacity(card_lines.len() + 1);
    padded.extend(card_lines.into_iter().map(pad_display_line));
    padded.push(styled_blank_line(width, padding_style));
    padded
}

#[cfg(test)]
#[path = "live_card_cache_tests.rs"]
mod tests;
