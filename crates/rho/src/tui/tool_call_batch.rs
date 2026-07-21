use std::collections::{BTreeMap, BTreeSet};

use rho_sdk::ToolCallId;

use super::{ToolEntry, ToolEntryState};

#[derive(Default)]
pub(super) struct ToolCallBatch {
    pub(super) previews: BTreeMap<usize, ToolEntry>,
    preview_call_ids: BTreeMap<ToolCallId, usize>,
    promoted_previews: BTreeSet<usize>,
    pub(super) running: BTreeMap<ToolCallId, ToolEntry>,
}

impl ToolCallBatch {
    pub(super) fn clear(&mut self) {
        self.previews.clear();
        self.preview_call_ids.clear();
        self.promoted_previews.clear();
        self.running.clear();
    }

    pub(super) fn is_running(&self) -> bool {
        !self.running.is_empty()
    }

    pub(super) fn live_entries(&self) -> impl Iterator<Item = &ToolEntry> {
        self.previews.values().chain(self.running.values())
    }

    pub(super) fn latest_mut(&mut self) -> Option<&mut ToolEntry> {
        self.running
            .last_entry()
            .map(|entry| entry.into_mut())
            .or_else(|| self.previews.last_entry().map(|entry| entry.into_mut()))
    }

    pub(super) fn started(&mut self, call_id: ToolCallId, display_lines: Vec<String>) {
        if let Some(index) = self.preview_call_ids.get(&call_id).copied() {
            self.previews.remove(&index);
            self.promoted_previews.insert(index);
        }
        self.running
            .insert(call_id, running_entry(display_lines, false));
    }

    pub(super) fn updated(&mut self, call_id: ToolCallId, display_lines: Vec<String>) {
        let expanded = self
            .running
            .get(&call_id)
            .is_some_and(|entry| entry.expanded);
        self.running
            .insert(call_id, running_entry(display_lines, expanded));
    }

    pub(super) fn preview(
        &mut self,
        index: usize,
        call_id: Option<ToolCallId>,
        display_lines: Vec<String>,
    ) {
        if self.promoted_previews.contains(&index) {
            return;
        }
        if let Some(call_id) = call_id {
            self.preview_call_ids.insert(call_id, index);
        }
        if display_lines.is_empty() {
            self.previews.remove(&index);
            return;
        }
        let expanded = self
            .previews
            .get(&index)
            .is_some_and(|entry| entry.expanded);
        self.previews
            .insert(index, running_entry(display_lines, expanded));
    }

    pub(super) fn finished(&mut self, call_id: &ToolCallId) -> bool {
        let expanded = self
            .running
            .remove(call_id)
            .is_some_and(|entry| entry.expanded);
        if let Some(index) = self.preview_call_ids.remove(call_id) {
            self.previews.remove(&index);
            self.promoted_previews.remove(&index);
        }
        expanded
    }
}

fn running_entry(display_lines: Vec<String>, expanded: bool) -> ToolEntry {
    ToolEntry {
        state: ToolEntryState::Running,
        display_lines,
        expanded,
        image: None,
    }
}
