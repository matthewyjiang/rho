//! The reasoning levels a session's model takes, which
//! [`crate::app::session_runtime::SessionRuntime::set_reasoning_level`]
//! applies, and their wire spelling.

use rho_protocol as wire;
use rho_providers::{
    model::{models_dev::cached_reasoning_capabilities, ReasoningCapabilities},
    reasoning::ReasoningLevel,
};

use crate::config::Config;

/// The reasoning choices of `config`'s model, at `current`: every level
/// the catalog lists for it (all of them when the catalog does not know),
/// lowest first, or `None` when its reasoning is not configurable, so hosts
/// do not render a one-entry picker that can never change.
pub(super) fn reasoning_options(
    config: &Config,
    current: ReasoningLevel,
) -> Option<wire::ReasoningOptions> {
    let capabilities = cached_reasoning_capabilities(&config.provider, &config.model);
    match capabilities {
        ReasoningCapabilities::NotConfigurable => None,
        ReasoningCapabilities::Unknown | ReasoningCapabilities::Levels(_) => {
            Some(wire::ReasoningOptions {
                current: reasoning_to_wire(current),
                available: capabilities
                    .selectable_levels(&ReasoningLevel::ALL, Some(current))
                    .into_iter()
                    .map(reasoning_to_wire)
                    .collect(),
            })
        }
    }
}

fn reasoning_to_wire(level: ReasoningLevel) -> wire::ReasoningLevel {
    match level {
        ReasoningLevel::Off => wire::ReasoningLevel::Off,
        ReasoningLevel::Minimal => wire::ReasoningLevel::Minimal,
        ReasoningLevel::Low => wire::ReasoningLevel::Low,
        ReasoningLevel::Medium => wire::ReasoningLevel::Medium,
        ReasoningLevel::High => wire::ReasoningLevel::High,
        ReasoningLevel::Xhigh => wire::ReasoningLevel::Xhigh,
        ReasoningLevel::Max => wire::ReasoningLevel::Max,
    }
}

/// The level `level` names, or `None` for one this rho does not know, such
/// as one a newer daemon or client sends. Its readers, the daemon and
/// `rho acp`, are unix-only (#1665).
#[cfg(unix)]
pub(crate) fn reasoning_from_wire(level: wire::ReasoningLevel) -> Option<ReasoningLevel> {
    match level {
        wire::ReasoningLevel::Off => Some(ReasoningLevel::Off),
        wire::ReasoningLevel::Minimal => Some(ReasoningLevel::Minimal),
        wire::ReasoningLevel::Low => Some(ReasoningLevel::Low),
        wire::ReasoningLevel::Medium => Some(ReasoningLevel::Medium),
        wire::ReasoningLevel::High => Some(ReasoningLevel::High),
        wire::ReasoningLevel::Xhigh => Some(ReasoningLevel::Xhigh),
        wire::ReasoningLevel::Max => Some(ReasoningLevel::Max),
        // `Unknown`, and any level a later protocol minor adds.
        _ => None,
    }
}

#[cfg(test)]
#[path = "reasoning_tests.rs"]
mod tests;
