//! The model setting type: a model table's `runtime`, `provider`, `model`,
//! and `auth` leaves, read, written, and carried over the protocol.

use rho_protocol::{ModelRuntime, SettingKind, SettingValue};
use toml_edit::Value;

use super::{path_starts_with, read_string, takes, SettingType};
use crate::config::document::ConfigDocument;

/// The runtimes a model setting may pick.
pub(crate) struct ModelSpec {
    pub(crate) runtimes: &'static [ModelRuntime],
}

/// The model a model setting holds: the `runtime`, `provider`, `model`, and
/// `auth` leaves of its table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelSetting {
    Rho {
        provider: String,
        /// A concrete model id, or `@alias`.
        model: String,
        auth: String,
    },
    ClaudeCli {
        model: Option<String>,
    },
}

const MODEL_LEAVES: [&str; 4] = ["runtime", "provider", "model", "auth"];

fn read_model(
    doc: &ConfigDocument,
    key: &str,
    spec: &ModelSpec,
) -> Result<Option<ModelSetting>, String> {
    if doc.get(key).is_some_and(|item| !item.is_table_like()) {
        return Err("must be a table with provider and model keys".into());
    }
    let leaf = |name: &str| read_string(doc, &format!("{key}.{name}"));
    let (runtime, provider, model, auth) = (
        leaf("runtime")?,
        leaf("provider")?,
        leaf("model")?,
        leaf("auth")?,
    );
    if runtime.is_none() && provider.is_none() && model.is_none() && auth.is_none() {
        return Ok(None);
    }
    let runtime_allowed = |runtime: ModelRuntime| spec.runtimes.contains(&runtime);
    match runtime.as_deref().unwrap_or(crate::config::RHO_RUNTIME_KEY) {
        crate::config::RHO_RUNTIME_KEY if runtime_allowed(ModelRuntime::Rho) => {
            Ok(Some(ModelSetting::Rho {
                provider: provider.unwrap_or_else(|| crate::config::DEFAULT_PROVIDER.into()),
                model: model.unwrap_or_else(|| crate::config::DEFAULT_MODEL.into()),
                auth: auth.unwrap_or_else(|| crate::config::DEFAULT_AUTH.into()),
            }))
        }
        crate::config::CLAUDE_CLI_RUNTIME_KEY if runtime_allowed(ModelRuntime::ClaudeCli) => {
            if provider.is_some() || auth.is_some() {
                return Err(format!(
                    "runtime {} takes no provider or auth",
                    crate::config::CLAUDE_CLI_RUNTIME_KEY
                ));
            }
            Ok(Some(ModelSetting::ClaudeCli { model }))
        }
        other => {
            let expected = spec
                .runtimes
                .iter()
                .filter_map(|runtime| match runtime {
                    ModelRuntime::Rho => Some(crate::config::RHO_RUNTIME_KEY),
                    ModelRuntime::ClaudeCli => Some(crate::config::CLAUDE_CLI_RUNTIME_KEY),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "runtime must be one of {expected}; got \"{other}\""
            ))
        }
    }
}

fn write_model(model: &ModelSetting, doc: &mut ConfigDocument, key: &str) {
    match model {
        ModelSetting::Rho {
            provider,
            model,
            auth,
        } => {
            doc.remove(&format!("{key}.runtime"));
            doc.set_value(&format!("{key}.provider"), Value::from(provider.as_str()));
            doc.set_value(&format!("{key}.model"), Value::from(model.as_str()));
            doc.set_value(&format!("{key}.auth"), Value::from(auth.as_str()));
        }
        ModelSetting::ClaudeCli { model } => {
            doc.set_value(
                &format!("{key}.runtime"),
                Value::from(crate::config::CLAUDE_CLI_RUNTIME_KEY),
            );
            doc.remove(&format!("{key}.provider"));
            doc.remove(&format!("{key}.auth"));
            match model {
                Some(model) => doc.set_value(&format!("{key}.model"), Value::from(model.as_str())),
                None => doc.remove(&format!("{key}.model")),
            }
        }
    }
}

fn model_wire(model: &ModelSetting) -> SettingValue {
    SettingValue::Model(match model {
        ModelSetting::Rho {
            provider, model, ..
        } => rho_protocol::ModelSelection::Rho {
            provider: provider.clone(),
            model: model.clone(),
        },
        ModelSetting::ClaudeCli { model } => rho_protocol::ModelSelection::ClaudeCli {
            model: model.clone(),
        },
    })
}

/// The protocol `value` as a model `spec` allows. A Rho model keeps
/// `current`'s auth while its provider stays, else takes the new provider's
/// default auth.
fn model_from_wire(
    value: &SettingValue,
    spec: &ModelSpec,
    current: Option<&ModelSetting>,
) -> Result<ModelSetting, String> {
    let SettingValue::Model(selection) = value else {
        return takes("a model");
    };
    let runtime = match selection {
        rho_protocol::ModelSelection::Rho { .. } => ModelRuntime::Rho,
        rho_protocol::ModelSelection::ClaudeCli { .. } => ModelRuntime::ClaudeCli,
    };
    if !spec.runtimes.contains(&runtime) {
        return Err("does not run on that runtime".into());
    }
    Ok(match selection {
        rho_protocol::ModelSelection::Rho { provider, model } => {
            let (current_provider, current_auth) = match current {
                Some(ModelSetting::Rho { provider, auth, .. }) => {
                    (provider.as_str(), auth.as_str())
                }
                _ => (crate::config::DEFAULT_PROVIDER, crate::config::DEFAULT_AUTH),
            };
            ModelSetting::Rho {
                provider: provider.clone(),
                model: model.clone(),
                auth: crate::config::inferred_provider_auth(
                    provider,
                    current_provider,
                    current_auth,
                ),
            }
        }
        rho_protocol::ModelSelection::ClaudeCli { model } => ModelSetting::ClaudeCli {
            model: model.clone(),
        },
    })
}

/// Removes a model's leaves, leaving the other settings of its table,
/// such as its reasoning.
fn remove_model(doc: &mut ConfigDocument, key: &str) {
    for leaf in MODEL_LEAVES {
        doc.remove(&format!("{key}.{leaf}"));
    }
}

fn owns_model_leaf(key: &str, path: &[String]) -> bool {
    let depth = key.split('.').count();
    path_starts_with(path, key)
        && (path.len() == depth
            || (path.len() == depth + 1 && MODEL_LEAVES.contains(&path[depth].as_str())))
}

impl SettingType for ModelSetting {
    type Spec = ModelSpec;
    fn kind(spec: &ModelSpec) -> SettingKind {
        SettingKind::Model {
            runtimes: spec.runtimes.to_vec(),
        }
    }
    fn read(doc: &ConfigDocument, key: &str, spec: &ModelSpec) -> Result<Option<Self>, String> {
        read_model(doc, key, spec)
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        write_model(self, doc, key);
    }
    fn remove(doc: &mut ConfigDocument, key: &str) {
        remove_model(doc, key);
    }
    fn owns(key: &str, path: &[String]) -> bool {
        owns_model_leaf(key, path)
    }
    fn to_wire(&self) -> Option<SettingValue> {
        Some(model_wire(self))
    }
    fn from_wire(value: &SettingValue, spec: &ModelSpec, current: &Self) -> Result<Self, String> {
        model_from_wire(value, spec, Some(current))
    }
}

/// An internal agent's model, unset until chosen. Unset, the agent has no
/// entry: its other keys, such as `kind`, only qualify a chosen model, and
/// any of them left behind would make an entry again.
impl SettingType for Option<ModelSetting> {
    type Spec = ModelSpec;
    fn kind(spec: &ModelSpec) -> SettingKind {
        ModelSetting::kind(spec)
    }
    fn read(doc: &ConfigDocument, key: &str, spec: &ModelSpec) -> Result<Option<Self>, String> {
        Ok(read_model(doc, key, spec)?.map(Some))
    }
    fn write(&self, doc: &mut ConfigDocument, key: &str) {
        match self {
            Some(model) => write_model(model, doc, key),
            None => doc.remove(key),
        }
    }
    fn remove(doc: &mut ConfigDocument, key: &str) {
        doc.remove(key);
    }
    fn owns(key: &str, path: &[String]) -> bool {
        owns_model_leaf(key, path)
    }
    fn to_wire(&self) -> Option<SettingValue> {
        self.as_ref().map(model_wire)
    }
    fn from_wire(value: &SettingValue, spec: &ModelSpec, current: &Self) -> Result<Self, String> {
        model_from_wire(value, spec, current.as_ref()).map(Some)
    }
}
