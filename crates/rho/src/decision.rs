//! Rho's side of the SDK decision protocol, [`rho_sdk::decision`]: the
//! decision model a config entry names, and a text model asked as one of
//! Rho's internal agents.

mod resolve;
mod text_model;

pub(crate) use resolve::{
    discovered_decision_model, entry_kind, kind_mismatch, resolve, ConfigError, EntryModel,
};
pub(crate) use text_model::TextModel;
