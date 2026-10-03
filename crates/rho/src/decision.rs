//! Rho's side of the SDK decision protocol, [`rho_sdk::decision`]: the
//! decision model a config entry names, and a text model asked as one of
//! Rho's internal agents.

mod resolve;
mod text_model;

pub(crate) use resolve::{resolve, ConfigError};
pub(crate) use text_model::TextModel;
