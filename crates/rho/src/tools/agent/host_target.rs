//! Chooses T3's `delegate_task` target for a child launched with a per-call model.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The parts of T3's `orchestrator_capabilities` reply that decide a target.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    inherited_provider_instance_id: String,
    #[serde(default)]
    inherited_options: Vec<SelectedOption>,
    providers: Vec<Provider>,
}

/// One setting T3 applies to a thread, such as its permission mode.
#[derive(Deserialize, Serialize)]
struct SelectedOption {
    id: String,
    value: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Provider {
    provider_instance_id: String,
    #[serde(default)]
    models: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
    #[serde(default)]
    options: Vec<ModelOption>,
}

#[derive(Deserialize)]
struct ModelOption {
    id: String,
    #[serde(default)]
    values: Vec<Value>,
}

/// T3's launch selection for a child whose `/agent:` rebind will select
/// `model`, so the child's thread shows the model it runs on.
///
/// `None` leaves T3 on the parent's selection. That is always safe, because the
/// rebind selects the model either way. A target is returned only when it
/// changes nothing else about the launch:
/// - T3 fails a launch whose model the parent's provider instance does not
///   offer (`model_unavailable`), so the model must be offered there.
/// - T3 keeps the parent's options, such as its permission mode and thought
///   level, only for an unchanged model. The target repeats them, and the
///   model must accept every one.
pub(super) fn launch_target(capabilities: &Value, model: &str) -> Option<Value> {
    let capabilities = Capabilities::deserialize(capabilities).ok()?;
    let offered = capabilities
        .providers
        .iter()
        .find(|provider| {
            provider.provider_instance_id == capabilities.inherited_provider_instance_id
        })?
        .models
        .iter()
        .find(|offered| offered.id == model)?;
    let accepts = |inherited: &SelectedOption| {
        offered
            .options
            .iter()
            .any(|option| option.id == inherited.id && option.values.contains(&inherited.value))
    };
    capabilities
        .inherited_options
        .iter()
        .all(accepts)
        .then(|| json!({"model": model, "options": capabilities.inherited_options}))
}

#[cfg(test)]
#[path = "host_target_tests.rs"]
mod tests;
