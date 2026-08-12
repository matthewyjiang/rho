//! Cursor Fast is a trailing `-fast` model-id suffix, not a service-tier field.
//!
//! Product names such as `grok-code-fast-1` keep `fast` in the middle of the id
//! and are not Fast variants. Auto routing (`auto` / wire `default`) has no Fast
//! suffix.

/// Whether `/fast` should rewrite this Cursor model id.
pub(crate) fn supports_fast_mode(model: &str) -> bool {
    let base = catalog_model_id(model);
    !matches!(base, "auto" | "default") && !base.contains("fast")
}

/// Catalog and config id: strip a trailing Fast variant suffix.
pub(crate) fn catalog_model_id(model: &str) -> &str {
    model
        .strip_suffix("-fast")
        .filter(|base| !base.is_empty())
        .unwrap_or(model)
}

/// Speed requested for one Cursor Run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorSpeed {
    Standard,
    Fast,
}

/// Wire model id sent on `AgentService/Run`.
pub(crate) fn wire_model_id(model: &str, speed: CursorSpeed) -> String {
    if model == "auto" {
        return "default".into();
    }
    let base = catalog_model_id(model);
    if speed == CursorSpeed::Fast && supports_fast_mode(model) {
        format!("{base}-fast")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
#[path = "fast_tests.rs"]
mod tests;
