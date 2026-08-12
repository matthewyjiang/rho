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
        let delta = Self {
            image_height: previous.max_image_height != next.max_image_height,
            tool_output: previous.max_tool_output_lines != next.max_tool_output_lines,
            zen: previous.zen_mode != next.zen_mode,
        };
        (delta.image_height || delta.tool_output || delta.zen).then_some(delta)
    }

    pub(super) fn image_only(self) -> bool {
        self.image_height && !self.tool_output && !self.zen
    }

    /// Entry indices that must be re-rendered for this soft delta.
    ///
    /// Image-height work uses the cache's tracked dependency indices so a
    /// text-only transcript does not walk every entry on a budget nudge.
    pub(super) fn resplice_indices(
        self,
        entries: &[Entry],
        image_height_deps: &[usize],
    ) -> Vec<usize> {
        if self.image_only() {
            return image_height_deps.to_vec();
        }

        let mut indices = Vec::new();
        if self.image_height {
            indices.extend_from_slice(image_height_deps);
        }
        if self.tool_output || self.zen {
            for (index, entry) in entries.iter().enumerate() {
                let needed = match entry {
                    Entry::Tool(_) => self.tool_output || self.zen,
                    Entry::Reasoning(_) => self.zen,
                    _ => false,
                };
                if needed {
                    indices.push(index);
                }
            }
        }
        indices.sort_unstable();
        indices.dedup();
        indices
    }
}
