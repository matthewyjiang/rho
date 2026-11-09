//! Client settings namespaces: each client registers its settings' specs,
//! and the daemon stores their values in `clients/<name>.toml`, one file per
//! client, so `config.toml` keeps its strict key check without knowing any
//! client's schema.
//!
//! A schema lives until the daemon stops, and a client registers again
//! after a restart. A key the file holds that the schema does not list is
//! left on disk and not served, so an older client cannot drop a newer
//! one's settings.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_protocol::{
    ClientName, ConfigError as WireError, ConfigGetResponse, ConfigRegisterParams, ConfigRevision,
    ConfigSetResponse, ConfigWatch, ErrorCode, RpcError, SettingChange, SettingSpec,
};
use tokio::sync::watch;

use super::{
    check_revision, file, file::Stat, read_for_change, refused, wire_errors, write_locked,
};
use crate::config::{client_specs_problem, spec_values, write_changes};

/// Every registered client's namespace.
pub(super) struct Clients {
    dir: PathBuf,
    registered: tokio::sync::Mutex<HashMap<ClientName, Client>>,
}

/// One registered client's schema and published state.
struct Client {
    specs: Vec<SettingSpec>,
    /// The file's `stat` at the last read; `None` before the first.
    seen: Option<Stat>,
    state: watch::Sender<Arc<ConfigGetResponse>>,
}

impl Clients {
    pub(super) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            registered: tokio::sync::Mutex::default(),
        }
    }

    /// Replaces the schema of `params.client`, then publishes its values
    /// under it.
    pub(super) async fn register(&self, params: ConfigRegisterParams) -> Result<(), RpcError> {
        let ConfigRegisterParams { client, settings } = params;
        if let Some(problem) = client_specs_problem(&settings) {
            return Err(RpcError::new(ErrorCode::InvalidParams, problem));
        }
        let path = self.path(&client);
        let mut registered = self.registered.lock().await;
        let entry = registered.entry(client).or_insert_with(|| Client {
            specs: Vec::new(),
            seen: None,
            state: watch::Sender::new(Arc::new(ConfigGetResponse {
                revision: ConfigRevision::new(""),
                values: Default::default(),
                errors: Vec::new(),
            })),
        });
        entry.specs = settings;
        entry.seen = None;
        entry.refresh(&path);
        Ok(())
    }

    pub(super) async fn get(&self, name: &ClientName) -> Result<ConfigWatch, RpcError> {
        let path = self.path(name);
        let mut registered = self.registered.lock().await;
        let client = registered.get_mut(name).ok_or_else(|| unregistered(name))?;
        client.refresh(&path);
        Ok(ConfigWatch::new(client.state.subscribe()))
    }

    pub(super) async fn set(
        &self,
        name: &ClientName,
        revision: &ConfigRevision,
        changes: &[SettingChange],
    ) -> Result<ConfigSetResponse, RpcError> {
        let path = self.path(name);
        let mut registered = self.registered.lock().await;
        let client = registered.get_mut(name).ok_or_else(|| unregistered(name))?;
        client.refresh(&path);
        check_revision(&client.state.borrow().revision, revision)?;
        let mut doc = read_for_change(&path)?;
        write_changes(&mut doc, &client.specs, changes).map_err(refused)?;
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            RpcError::new(
                ErrorCode::InternalError,
                format!("could not create {}: {error}", self.dir.display()),
            )
        })?;
        write_locked(&path, &doc)?;
        client.seen = None;
        let revision = client.refresh(&path);
        Ok(ConfigSetResponse { revision })
    }

    fn path(&self, name: &ClientName) -> PathBuf {
        self.dir.join(format!("{name}.toml"))
    }
}

impl Client {
    /// Reads the client's file again if it changed since the last read and
    /// publishes its values. Returns the revision now in force.
    fn refresh(&mut self, path: &Path) -> ConfigRevision {
        let stat = Stat::of(path);
        if self.seen == Some(stat) {
            return self.state.borrow().revision.clone();
        }
        self.seen = Some(stat);
        let (values, errors) = match file::read(path) {
            Ok(doc) => {
                let (values, errors) = spec_values(&doc, &self.specs);
                (values, wire_errors(&errors))
            }
            Err(error) => {
                let (values, _) = spec_values(&Default::default(), &self.specs);
                let error = WireError {
                    key: None,
                    message: error.message(path),
                };
                (values, vec![error])
            }
        };
        let state = ConfigGetResponse {
            revision: file::revision(&values),
            values,
            errors,
        };
        let revision = state.revision.clone();
        self.state.send_if_modified(|current| {
            let changed = **current != state;
            if changed {
                *current = Arc::new(state);
            }
            changed
        });
        revision
    }
}

fn unregistered(name: &ClientName) -> RpcError {
    RpcError::new(
        ErrorCode::InvalidParams,
        format!("client `{name}` has not registered its settings"),
    )
}
