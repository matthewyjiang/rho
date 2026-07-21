use std::collections::{BTreeMap, BTreeSet};

use rho_sdk::ToolCallId;

use super::{ToolEntry, ToolEntryState};

#[derive(Clone)]
enum LiveToolKey {
    Preview(usize),
    Running(ToolCallId),
}

#[derive(Default)]
pub(super) struct ToolCallBatch {
    pub(super) previews: BTreeMap<usize, ToolEntry>,
    preview_call_ids: BTreeMap<ToolCallId, usize>,
    promoted_previews: BTreeSet<usize>,
    pub(super) running: BTreeMap<ToolCallId, ToolEntry>,
    model_order: BTreeMap<usize, LiveToolKey>,
    unindexed_running_order: Vec<ToolCallId>,
}

impl ToolCallBatch {
    pub(super) fn clear(&mut self) {
        self.previews.clear();
        self.preview_call_ids.clear();
        self.promoted_previews.clear();
        self.running.clear();
        self.model_order.clear();
        self.unindexed_running_order.clear();
    }

    pub(super) fn is_running(&self) -> bool {
        !self.running.is_empty()
    }

    pub(super) fn live_entries(&self) -> impl Iterator<Item = &ToolEntry> {
        self.model_order
            .values()
            .filter_map(|key| match key {
                LiveToolKey::Preview(index) => self.previews.get(index),
                LiveToolKey::Running(call_id) => self.running.get(call_id),
            })
            .chain(
                self.unindexed_running_order
                    .iter()
                    .filter_map(|call_id| self.running.get(call_id)),
            )
    }

    pub(super) fn latest_mut(&mut self) -> Option<&mut ToolEntry> {
        let key = self
            .unindexed_running_order
            .last()
            .cloned()
            .map(LiveToolKey::Running)
            .or_else(|| {
                self.model_order
                    .last_key_value()
                    .map(|(_, key)| key.clone())
            })?;
        match key {
            LiveToolKey::Preview(index) => self.previews.get_mut(&index),
            LiveToolKey::Running(call_id) => self.running.get_mut(&call_id),
        }
    }

    pub(super) fn started(&mut self, call_id: ToolCallId, display_lines: Vec<String>) {
        if let Some(index) = self.preview_call_ids.get(&call_id).copied() {
            self.previews.remove(&index);
            self.promoted_previews.insert(index);
            self.model_order
                .insert(index, LiveToolKey::Running(call_id.clone()));
            self.unindexed_running_order
                .retain(|running_id| running_id != &call_id);
        } else if !self.running.contains_key(&call_id) {
            self.unindexed_running_order.push(call_id.clone());
        }
        self.running
            .insert(call_id, running_entry(display_lines, false));
    }

    pub(super) fn updated(&mut self, call_id: ToolCallId, display_lines: Vec<String>) {
        let expanded = self
            .running
            .get(&call_id)
            .is_some_and(|entry| entry.expanded);
        if !self.running.contains_key(&call_id) {
            self.unindexed_running_order.push(call_id.clone());
        }
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
            self.preview_call_ids
                .retain(|_, existing_index| *existing_index != index);
            self.preview_call_ids.insert(call_id, index);
        }
        if display_lines.is_empty() {
            self.previews.remove(&index);
            self.model_order.remove(&index);
            return;
        }
        let expanded = self
            .previews
            .get(&index)
            .is_some_and(|entry| entry.expanded);
        self.previews
            .insert(index, running_entry(display_lines, expanded));
        self.model_order.insert(index, LiveToolKey::Preview(index));
    }

    pub(super) fn finished(&mut self, call_id: &ToolCallId) -> bool {
        let expanded = self
            .running
            .remove(call_id)
            .is_some_and(|entry| entry.expanded);
        self.model_order
            .retain(|_, key| !matches!(key, LiveToolKey::Running(id) if id == call_id));
        self.unindexed_running_order
            .retain(|running_id| running_id != call_id);
        if let Some(index) = self.preview_call_ids.remove(call_id) {
            self.previews.remove(&index);
            self.promoted_previews.remove(&index);
            self.model_order.remove(&index);
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

#[cfg(test)]
#[path = "tool_call_batch_tests.rs"]
mod tests;
