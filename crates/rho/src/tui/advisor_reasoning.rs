//! Advisor reasoning: the picker `/advisor model` opens after a model is
//! chosen, and the one apply path shared with the `/config` reasoning row.

use rho_providers::{model::ReasoningCapabilities, reasoning::ReasoningLevel};

use crate::{
    agent::{internal_agent_reasoning_capabilities, ADVISOR_AGENT_ID},
    tools::advisor::advisor_effective_reasoning,
};

use super::{
    advisor_command::AdvisorRuntime, App, ComposerMode, PickerBadge, PickerBadgeTone, PickerItem,
    UiPicker,
};

/// Levels the advisor reasoning picker offers, or `None` when it is skipped.
///
/// Only advertised levels count. Unknown capabilities skip the picker instead
/// of guessing levels the model may reject, and a single level is no choice;
/// the runtime normalizes a carried level onto it. The `/config` row still
/// cycles unknown capabilities, because it is an explicit request to change
/// reasoning rather than an unprompted follow-up.
pub(super) fn offered_reasoning_levels(
    capabilities: &ReasoningCapabilities,
) -> Option<&[ReasoningLevel]> {
    capabilities.levels().filter(|levels| levels.len() > 1)
}

fn advisor_reasoning_picker(levels: &[ReasoningLevel], current: ReasoningLevel) -> UiPicker {
    let items = levels
        .iter()
        .map(|level| PickerItem {
            section: None,
            label: level.to_string(),
            detail: None,
            preview: None,
            badge: (*level == current).then(|| PickerBadge {
                text: "selected".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: level.to_string(),
            selection_verb: None,
            allow_filter_completion: true,
            search_terms: Vec::new(),
        })
        .collect();
    let mut picker =
        UiPicker::advisor_reasoning(format!("select reasoning for {ADVISOR_AGENT_ID}"), items);
    picker.selected = levels
        .iter()
        .position(|level| *level == current)
        .unwrap_or(0);
    picker
}

impl App {
    /// Opens the advisor reasoning picker alone in the composer. Reports
    /// whether it opened; it stays closed when the advisor model offers no
    /// reasoning choice.
    pub(super) fn open_advisor_reasoning_picker(&mut self) -> bool {
        let Some(selection) = self.info.runtime.internal_agents.get(ADVISOR_AGENT_ID) else {
            return false;
        };
        let capabilities = internal_agent_reasoning_capabilities(selection);
        let Some(levels) = offered_reasoning_levels(&capabilities) else {
            return false;
        };
        let picker = advisor_reasoning_picker(levels, advisor_effective_reasoning(selection));
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        true
    }

    pub(super) async fn commit_advisor_reasoning(
        &mut self,
        value: &str,
        agent: &mut impl AdvisorRuntime,
    ) -> anyhow::Result<()> {
        // Rows carry `ReasoningLevel` display text, so this always parses.
        let Ok(reasoning) = value.parse::<ReasoningLevel>() else {
            return Ok(());
        };
        self.apply_advisor_reasoning(reasoning, agent).await
    }

    /// Saves an advisor reasoning level and applies it to the live advisor
    /// when the mode is on.
    pub(super) async fn apply_advisor_reasoning(
        &mut self,
        reasoning: ReasoningLevel,
        agent: &mut impl AdvisorRuntime,
    ) -> anyhow::Result<()> {
        self.set_advisor_reasoning(reasoning)?;
        if self.info.runtime.advisor_mode {
            self.sync_advisor_runtime(agent).await;
        }
        Ok(())
    }

    /// Esc on the reasoning picker: the model chosen just before stays saved
    /// with the reasoning it carried over.
    pub(super) fn report_advisor_reasoning_unchanged(&mut self) {
        let status = match self.info.runtime.internal_agents.get(ADVISOR_AGENT_ID) {
            Some(selection) => format!(
                "advisor reasoning unchanged: {}",
                advisor_effective_reasoning(selection)
            ),
            None => "advisor reasoning unchanged".into(),
        };
        self.set_status(status);
    }
}

#[cfg(test)]
#[path = "advisor_reasoning_tests.rs"]
mod tests;
