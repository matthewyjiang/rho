//! Cursor Fast is a trailing `-fast` model-id suffix, not a service-tier field.
//!
//! Product names such as `grok-code-fast-1` keep `fast` in the middle of the id
//! and are not Fast variants. Auto routing (`auto` / wire `default`) has no Fast
//! suffix. Effort suffixes are stripped from catalog ids and reapplied on the wire.

use super::effort::{split_effort, CursorEffort};

/// Whether `/fast` should rewrite this Cursor model id.
pub(crate) fn supports_fast_mode(model: &str) -> bool {
    let base = catalog_model_id(model);
    !matches!(base, "auto" | "default") && !base.contains("fast")
}

/// Catalog and config id: strip trailing Fast and effort variant suffixes.
pub(crate) fn catalog_model_id(model: &str) -> &str {
    split_effort(strip_fast_suffix(model).0).0
}

/// Speed requested for one Cursor Run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorSpeed {
    Standard,
    Fast,
}

/// Wire model id sent on `AgentService/Run`.
pub(crate) fn wire_model_id(model: &str, speed: CursorSpeed, effort: CursorEffort) -> String {
    if model == "auto" {
        return "default".into();
    }
    let (without_fast, _) = strip_fast_suffix(model);
    let (base, baked) = split_effort(without_fast);
    let token = match effort {
        CursorEffort::Unspecified => baked.and_then(crate::reasoning::ReasoningLevel::effort),
        CursorEffort::Level(level) => level
            .effort()
            .or_else(|| baked.and_then(crate::reasoning::ReasoningLevel::effort)),
    };
    let mut id = match token {
        Some(token) => format!("{base}-{token}"),
        None => base.to_string(),
    };
    if speed == CursorSpeed::Fast && supports_fast_mode(model) {
        id.push_str("-fast");
    }
    id
}

pub(crate) fn strip_fast_suffix(model: &str) -> (&str, bool) {
    match model.strip_suffix("-fast").filter(|base| !base.is_empty()) {
        Some(base) if !base.contains("fast") => (base, true),
        _ => (model, false),
    }
}

#[cfg(test)]
#[path = "fast_tests.rs"]
mod tests;
