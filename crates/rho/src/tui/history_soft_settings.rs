use super::{history_cache::HistoryRenderSettings, Entry};

/// Soft layout knobs that can update discrete entries without dropping the
/// whole transcript suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SoftSettingsDelta {
    pub(super) image_height: bool,
    pub(super) tool_output: bool,
    pub(super) zen: bool,
}

impl SoftSettingsDelta {
    pub(super) fn between(
        previous: HistoryRenderSettings,
        next: HistoryRenderSettings,
    ) -> Option<Self> {
        if previous.requires_full_rebuild(next) || previous == next {
            return None;
        }
        Some(Self {
            image_height: previous.max_image_height != next.max_image_height,
            tool_output: previous.max_tool_output_lines != next.max_tool_output_lines,
            zen: previous.zen_mode != next.zen_mode,
        })
    }

    pub(super) fn image_only(self) -> bool {
        self.image_height && !self.tool_output && !self.zen
    }

    /// Whether this history entry's rendered height depends on the soft knobs
    /// in this delta (excluding image-height, which uses cached dependency flags).
    pub(super) fn needs_entry(self, entry: &Entry) -> bool {
        match entry {
            Entry::Tool(_) => self.tool_output || self.zen,
            Entry::Reasoning(_) => self.zen,
            Entry::User(_)
            | Entry::Assistant(_)
            | Entry::Notice(_)
            | Entry::RuntimeInfo(_)
            | Entry::Changelog(_)
            | Entry::UsageLimits(_)
            | Entry::Error(_) => false,
        }
    }
}
