use std::collections::BTreeMap;

use rho_sdk::ToolCallId;

use rho_tools::tool_card::{ToolCard, ToolStatus};

use super::ToolEntry;

#[derive(Clone)]
enum LiveToolKey {
    Preview(usize),
    Running(ToolCallId),
}

#[derive(Default)]
pub(super) struct ToolCallBatch {
    pub(super) previews: BTreeMap<usize, ToolEntry>,
    /// Once a call id is known, it owns one slot for the rest of the preview life.
    preview_call_ids: BTreeMap<ToolCallId, usize>,
    pub(super) running: BTreeMap<ToolCallId, ToolEntry>,
    model_order: BTreeMap<usize, LiveToolKey>,
    unindexed_running_order: Vec<ToolCallId>,
}

impl ToolCallBatch {
    pub(super) fn clear(&mut self) {
        self.previews.clear();
        self.preview_call_ids.clear();
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

    pub(super) fn interrupted_entries(&self) -> Vec<ToolEntry> {
        self.live_entries()
            .cloned()
            .map(|mut entry| {
                entry.card.status = ToolStatus::Interrupted;
                entry
            })
            .collect()
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

    pub(super) fn started(&mut self, call_id: ToolCallId, card: ToolCard) {
        if let Some(index) = self.preview_call_ids.remove(&call_id) {
            self.previews.remove(&index);
            self.model_order
                .insert(index, LiveToolKey::Running(call_id.clone()));
            self.unindexed_running_order
                .retain(|running_id| running_id != &call_id);
        } else if !self.running.contains_key(&call_id) {
            self.unindexed_running_order.push(call_id.clone());
        }
        self.running
            .insert(call_id, running_entry(card, /*expanded*/ false));
    }

    pub(super) fn updated(&mut self, call_id: ToolCallId, card: ToolCard) {
        let expanded = self
            .running
            .get(&call_id)
            .is_some_and(|entry| entry.expanded);
        if !self.running.contains_key(&call_id) {
            self.unindexed_running_order.push(call_id.clone());
        }
        self.running.insert(call_id, running_entry(card, expanded));
    }

    /// Stream preview addressed by provider output index.
    ///
    /// When `call_id` is known it owns the slot: later stream or proposal traffic
    /// for that id updates the same card even if indexes differ.
    pub(super) fn preview(
        &mut self,
        index: usize,
        call_id: Option<ToolCallId>,
        card: Option<ToolCard>,
    ) {
        if call_id
            .as_ref()
            .is_some_and(|id| self.running.contains_key(id))
        {
            return;
        }
        let slot = call_id
            .as_ref()
            .and_then(|id| self.preview_call_ids.get(id).copied())
            .unwrap_or(index);
        if matches!(self.model_order.get(&slot), Some(LiveToolKey::Running(_))) {
            return;
        }
        if let Some(call_id) = call_id {
            self.bind_call_id(call_id, slot);
        }
        if let Some(card) = card {
            self.write_preview(slot, card);
        } else if self.previews.contains_key(&slot) {
            // Identity-only bind: keep the existing card and model order slot.
            self.model_order.insert(slot, LiveToolKey::Preview(slot));
        }
    }

    /// Proposal preview addressed only by call id.
    ///
    /// Reuses the stream slot when the id already appeared; otherwise appends a
    /// new slot. Does not invent a dense index in the provider namespace.
    pub(super) fn preview_call(&mut self, call_id: ToolCallId, card: ToolCard) {
        if self.running.contains_key(&call_id) {
            return;
        }
        let slot = self
            .preview_call_ids
            .get(&call_id)
            .copied()
            .unwrap_or_else(|| self.next_slot());
        self.bind_call_id(call_id, slot);
        self.write_preview(slot, card);
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
            self.model_order.remove(&index);
        }
        expanded
    }

    fn bind_call_id(&mut self, call_id: ToolCallId, slot: usize) {
        if let Some(previous) = self.preview_call_ids.insert(call_id.clone(), slot) {
            if previous != slot {
                self.previews.remove(&previous);
                self.model_order.remove(&previous);
            }
        }
        self.preview_call_ids
            .retain(|id, existing| *id == call_id || *existing != slot);
    }

    fn write_preview(&mut self, slot: usize, card: ToolCard) {
        let expanded = self.previews.get(&slot).is_some_and(|entry| entry.expanded);
        self.previews.insert(slot, running_entry(card, expanded));
        self.model_order.insert(slot, LiveToolKey::Preview(slot));
    }

    fn next_slot(&self) -> usize {
        let max_order = self.model_order.keys().next_back().copied();
        let max_preview = self.previews.keys().next_back().copied();
        max_order
            .max(max_preview)
            .map(|index| index + 1)
            .unwrap_or(0)
    }
}

fn running_entry(card: ToolCard, expanded: bool) -> ToolEntry {
    ToolEntry {
        card,
        expanded,
        image: None,
    }
}

#[cfg(test)]
#[path = "tool_call_batch_tests.rs"]
mod tests;
