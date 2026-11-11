//! How TUI chrome names a model: the catalog name a person recognizes
//! (`Claude Opus 5.5`) when one is known, the model id otherwise.
//!
//! A daemon session's own labels decide for the models it offers, because they
//! are the names its model picker shows. The local catalog answers for models
//! the daemon does not offer and for in-process sessions. A model keeps its id
//! when nothing names it or when its name is shared with another model from
//! the same provider, never a name guessed from it.

use rho_protocol::ModelChoice;
use rho_providers::{
    model::display_name::unambiguous_model_display_name, provider::model_reference,
};

use super::App;
use crate::config::{InternalAgentModelConfig, InternalAgentTarget};

/// Names models for status, divider, and notice text.
///
/// `offered` is the daemon session's model choices; empty in process.
#[derive(Clone, Copy, Default)]
pub(super) struct ModelNames<'a> {
    offered: &'a [ModelChoice],
}

impl<'a> ModelNames<'a> {
    pub(super) fn new(offered: &'a [ModelChoice]) -> Self {
        Self { offered }
    }

    /// The display name of `provider/model`, or `None` when the id should
    /// show instead.
    pub(super) fn name(&self, provider: &str, model: &str) -> Option<String> {
        match self
            .offered
            .iter()
            .find(|choice| choice.model.provider == provider && choice.model.model == model)
        {
            // The daemon labels a model with its id when no catalog names it,
            // or when another model from its provider shares its name; the id
            // carries its provider when another provider offers the same one.
            // The catalog would say no more in any of these cases.
            Some(choice) => {
                let is_id =
                    choice.label == model || choice.label == format!("{model} ({provider})");
                (!is_id).then(|| choice.label.clone())
            }
            None => unambiguous_model_display_name(provider, model),
        }
    }

    /// The display name, or `provider/model` when nothing names it.
    pub(super) fn reference(&self, provider: &str, model: &str) -> String {
        self.name(provider, model)
            .unwrap_or_else(|| model_reference(provider, model))
    }

    /// An internal agent's model, as [`Self::reference`] names it. A Claude
    /// Code model keeps its `claude-code/<model>` form: Claude Code resolves
    /// that name, not Rho's catalogs.
    pub(super) fn internal_agent(&self, selection: &InternalAgentModelConfig) -> String {
        match &selection.target {
            InternalAgentTarget::Rho(rho) => self.reference(&rho.provider, &rho.model),
            InternalAgentTarget::ClaudeCli { .. } => selection.display_reference(),
        }
    }
}

impl App {
    /// Model names for this session: the daemon's offered labels, when the
    /// session runs on a daemon, ahead of the local catalog.
    pub(super) fn model_names(&self) -> ModelNames<'_> {
        ModelNames::new(
            self.daemon_options
                .as_ref()
                .map_or(&[], |options| options.model.available.as_slice()),
        )
    }
}

#[cfg(test)]
#[path = "model_names_tests.rs"]
pub(super) mod tests;
