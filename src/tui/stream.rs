use super::{
    markdown::{markdown_rendered_width, markdown_stream_prefix},
    render::{complete_visual_prefix, display_width},
};

#[derive(Debug, Default)]
pub(super) struct AppendOnlyStream {
    pending: String,
    emitted_text: String,
    leading_blank_emitted: bool,
    previous_emission_ended_at_wrap: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct StreamFragment {
    text: String,
    include_leading_blank: bool,
    skip_leading_newline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderableSplit {
    byte_index: usize,
    skip_leading_newline: bool,
    ends_with_wrap: bool,
}

impl AppendOnlyStream {
    pub(super) fn reset(&mut self) {
        self.pending.clear();
        self.emitted_text.clear();
        self.leading_blank_emitted = false;
        self.previous_emission_ended_at_wrap = false;
    }

    pub(super) fn push_delta(&mut self, delta: &str) {
        self.pending.push_str(delta);
    }

    pub(super) fn pending_text(&self) -> &str {
        &self.pending
    }

    pub(super) fn drain_renderable(&mut self, inner_width: usize) -> Option<StreamFragment> {
        self.drain_renderable_with_prefix(inner_width, |_pending, byte_index| byte_index)
    }

    pub(super) fn drain_renderable_markdown(
        &mut self,
        inner_width: usize,
        in_code_block: bool,
    ) -> Option<StreamFragment> {
        let split = self.markdown_renderable_split(inner_width, in_code_block, false)?;
        Some(self.take_pending_prefix(
            split.byte_index,
            split.skip_leading_newline,
            split.ends_with_wrap,
        ))
    }

    pub(super) fn drain_preview(&mut self) -> Option<StreamFragment> {
        self.drain_preview_with_width(|text| display_width(text))
    }

    pub(super) fn drain_preview_markdown(
        &mut self,
        inner_width: usize,
        in_code_block: bool,
    ) -> Option<StreamFragment> {
        let split = self.markdown_renderable_split(inner_width, in_code_block, true)?;
        Some(self.take_pending_prefix(
            split.byte_index,
            split.skip_leading_newline,
            split.ends_with_wrap,
        ))
    }

    pub(super) fn finish(&mut self) -> Option<StreamFragment> {
        if self.pending.is_empty() {
            return None;
        }
        let skip_leading_newline = self.should_skip_leading_newline();
        Some(self.take_pending_prefix(self.pending.len(), skip_leading_newline, false))
    }

    pub(super) fn emitted_text(&self) -> &str {
        &self.emitted_text
    }

    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.emitted_text.is_empty()
    }

    fn should_skip_leading_newline(&self) -> bool {
        self.previous_emission_ended_at_wrap && self.pending.starts_with('\n')
    }

    fn markdown_renderable_split(
        &self,
        inner_width: usize,
        in_code_block: bool,
        allow_partial_line: bool,
    ) -> Option<RenderableSplit> {
        let skip_leading_newline = self.should_skip_leading_newline();
        let scan_start = usize::from(skip_leading_newline);
        let pending = &self.pending[scan_start..];
        let prefix = markdown_stream_prefix(pending, inner_width, in_code_block);
        let mut renderable_byte_index = prefix.byte_index;
        let mut ends_with_wrap = prefix.ends_with_wrap;
        if allow_partial_line && renderable_byte_index == 0 {
            if let Some(byte_index) = self.preview_byte_index(pending, |text| {
                markdown_rendered_width(text, inner_width, in_code_block)
            }) {
                renderable_byte_index = byte_index;
                ends_with_wrap = false;
            }
        }
        let split_at = scan_start + renderable_byte_index;
        if split_at == 0 {
            return None;
        }
        Some(RenderableSplit {
            byte_index: split_at,
            skip_leading_newline,
            ends_with_wrap,
        })
    }

    fn drain_preview_with_width(
        &mut self,
        rendered_width: impl Fn(&str) -> usize,
    ) -> Option<StreamFragment> {
        let skip_leading_newline = self.should_skip_leading_newline();
        let scan_start = usize::from(skip_leading_newline);
        let pending = &self.pending[scan_start..];
        let split_at = scan_start + self.preview_byte_index(pending, rendered_width)?;
        Some(self.take_pending_prefix(split_at, skip_leading_newline, false))
    }

    fn preview_byte_index(
        &self,
        pending: &str,
        rendered_width: impl Fn(&str) -> usize,
    ) -> Option<usize> {
        if pending.is_empty() || pending.starts_with('\n') {
            return None;
        }
        let line_end = pending.find('\n').unwrap_or(pending.len());
        let current_line = &pending[..line_end];
        let width = rendered_width(current_line);
        (width > 0).then_some(line_end)
    }

    fn drain_renderable_with_prefix(
        &mut self,
        inner_width: usize,
        renderable_prefix: impl FnOnce(&str, usize) -> usize,
    ) -> Option<StreamFragment> {
        let skip_leading_newline = self.should_skip_leading_newline();
        let scan_start = usize::from(skip_leading_newline);
        let pending = &self.pending[scan_start..];
        let prefix = complete_visual_prefix(pending, inner_width);
        let renderable_byte_index = renderable_prefix(pending, prefix.byte_index);
        let split_at = scan_start + renderable_byte_index;
        if split_at == 0 {
            return None;
        }
        Some(self.take_pending_prefix(
            split_at,
            skip_leading_newline,
            prefix.ends_with_wrap && renderable_byte_index == prefix.byte_index,
        ))
    }

    fn take_pending_prefix(
        &mut self,
        byte_index: usize,
        skip_leading_newline: bool,
        ends_with_wrap: bool,
    ) -> StreamFragment {
        let text: String = self.pending.drain(..byte_index).collect();
        let include_leading_blank = !self.leading_blank_emitted;
        self.leading_blank_emitted = true;
        self.previous_emission_ended_at_wrap = ends_with_wrap;
        self.emitted_text.push_str(&text);
        StreamFragment {
            text,
            include_leading_blank,
            skip_leading_newline,
        }
    }
}

impl StreamFragment {
    pub(super) fn render_text(&self) -> &str {
        if self.skip_leading_newline {
            &self.text['\n'.len_utf8()..]
        } else {
            &self.text
        }
    }

    pub(super) fn into_text(self) -> String {
        self.text
    }

    pub(super) fn include_leading_blank(&self) -> bool {
        self.include_leading_blank
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
