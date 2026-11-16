//! The followed session's A2UI processor, and which call each surface is
//! drawn under.
//!
//! The daemon checked every message before sending it, and resends a
//! call's surface with each update of the call, so the store applies a
//! call's messages once and trusts them. Nothing here outlasts a reset:
//! the replay shows every call again, in order.

use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
    time::SystemTime,
};

use rho_a2ui::{
    Action, Catalog, Dispatched, Functions, Interaction, MessageProcessor, Pointer, UserActivation,
    Validation,
};
use rho_sdk::ToolCallId;
use serde_json::Value;

use super::tree::{self, LocalState, SurfaceTree, Target};

pub(super) struct SurfaceStore {
    processor: MessageProcessor,
    /// The calls whose messages the processor has.
    applied: BTreeSet<ToolCallId>,
    /// Each surface a call showed and the latest call that showed it, the
    /// one whose card it is drawn under; most recently shown last.
    shown: Vec<(String, ToolCallId)>,
    /// What the user chose on each surface that its data model does not
    /// hold.
    local: HashMap<String, LocalState>,
}

impl Default for SurfaceStore {
    fn default() -> Self {
        let mut processor = MessageProcessor::new(Validation::Skip);
        processor
            .add_catalog(Catalog::basic().clone(), Functions::basic())
            .expect("the basic catalog compiles");
        Self {
            processor,
            applied: BTreeSet::new(),
            shown: Vec::new(),
            local: HashMap::new(),
        }
    }
}

impl SurfaceStore {
    /// Applies the `messages` call `call` carries about `surface`, unless
    /// it has them already, and says whether it applied them.
    pub(super) fn apply(&mut self, call: &ToolCallId, surface: &str, messages: &[Value]) -> bool {
        if !self.applied.insert(call.clone()) {
            return false;
        }
        let processed = self.processor.process(&Value::Array(messages.to_vec()));
        for error in processed.errors() {
            tracing::debug!(surface, error = error.message(), "a2ui message refused");
        }
        self.shown.retain(|(shown, _)| shown != surface);
        self.shown.push((surface.to_owned(), call.clone()));
        true
    }

    /// The call whose card `surface` is drawn under, while it exists.
    pub(super) fn drawn_under(&self, surface: &str) -> Option<&ToolCallId> {
        self.processor.surface(surface)?;
        self.shown
            .iter()
            .find_map(|(shown, call)| (shown == surface).then_some(call))
    }

    /// The surfaces that exist, the one shown last first.
    pub(super) fn latest_first(&self) -> impl Iterator<Item = &str> {
        self.shown
            .iter()
            .rev()
            .map(|(surface, _)| surface.as_str())
            .filter(|surface| self.processor.surface(surface).is_some())
    }

    pub(super) fn tree(&self, surface: &str) -> Option<SurfaceTree> {
        static NONE: LazyLock<LocalState> = LazyLock::new(LocalState::new);
        let local = self.local.get(surface).unwrap_or(&NONE);
        tree::build(&self.processor, surface, local)
    }

    /// The value at `binding` in `surface`'s data model.
    pub(super) fn read(&self, surface: &str, binding: &Pointer) -> Option<&Value> {
        self.processor.surface(surface)?.data().get(binding)
    }

    /// Writes what the user entered at `binding`, as a two-way binding
    /// does.
    pub(super) fn write(&mut self, surface: &str, binding: &Pointer, value: Value) {
        let Some(surface) = self.processor.surface_mut(surface) else {
            return;
        };
        if let Err(error) = surface.set_data(binding, value) {
            tracing::debug!(%error, "a surface's data model refused a write");
        }
    }

    /// What the user chose on `target` that the data model does not hold:
    /// its tab, or whether its modal is open.
    pub(super) fn local(&self, surface: &str, target: &Target) -> usize {
        self.local
            .get(surface)
            .and_then(|local| local.get(target))
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn set_local(&mut self, surface: &str, target: &Target, value: usize) {
        self.local
            .entry(surface.to_owned())
            .or_default()
            .insert(target.clone(), value);
    }

    /// Runs `action` of `target`, which the user just activated.
    pub(super) fn dispatch(
        &mut self,
        surface: &str,
        target: &Target,
        action: &Action,
    ) -> Option<Dispatched> {
        self.processor.dispatch_action(
            &Interaction {
                surface_id: surface,
                source_component_id: &target.component,
                scope: target.scope.clone(),
                activation: UserActivation::Present,
                metadata: None,
                at: SystemTime::now(),
            },
            action,
        )
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
