//! How `/fast` changes a supported model.
//!
//! The command is shared. Codex sends `service_tier: "priority"` and keeps the
//! model id. xAI OAuth `grok-4.7` keeps that id in the session and sends
//! `grok-4.7-build-fast` on the request instead.

use super::openai;

pub const GROK_4_7: &str = "grok-4.7";
pub const GROK_4_7_BUILD_FAST: &str = "grok-4.7-build-fast";

/// Provider-specific fast serving for one selected model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastServing {
    /// Codex priority tier. The request model id does not change.
    PriorityTier,
    /// Send this model id. The selected model stays unchanged.
    RequestModel(&'static str),
}

/// Returns how `/fast` is implemented for this selection, if at all.
pub fn fast_serving(provider: &str, model: &str, auth: &str) -> Option<FastServing> {
    if openai::supports_fast_mode(provider, model) {
        return Some(FastServing::PriorityTier);
    }
    if provider == "xai" && model == GROK_4_7 && auth == "xai-oauth" {
        return Some(FastServing::RequestModel(GROK_4_7_BUILD_FAST));
    }
    None
}

pub fn supports_fast_mode(provider: &str, model: &str, auth: &str) -> bool {
    fast_serving(provider, model, auth).is_some()
}

/// Model id to put on the request when `fast` is the saved preference.
///
/// Selections that do not implement fast mode as a different model id return
/// `model` unchanged, including when fast mode is off.
pub fn request_model<'a>(provider: &str, model: &'a str, auth: &str, fast: bool) -> &'a str {
    if fast {
        if let Some(FastServing::RequestModel(wire)) = fast_serving(provider, model, auth) {
            return wire;
        }
    }
    model
}

#[cfg(test)]
#[path = "fast_mode_tests.rs"]
mod tests;
