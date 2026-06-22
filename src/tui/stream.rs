use super::render::complete_visual_prefix;

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

    pub(super) fn drain_renderable(&mut self, inner_width: usize) -> Option<StreamFragment> {
        let skip_leading_newline = self.should_skip_leading_newline();
        let scan_start = usize::from(skip_leading_newline);
        let prefix = complete_visual_prefix(&self.pending[scan_start..], inner_width);
        let split_at = scan_start + prefix.byte_index;
        if split_at == 0 {
            return None;
        }
        Some(self.take_pending_prefix(split_at, skip_leading_newline, prefix.ends_with_wrap))
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
