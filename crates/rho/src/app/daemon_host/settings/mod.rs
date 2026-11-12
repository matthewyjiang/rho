//! The daemon's settings (#1720): the agent's `config.toml` and each
//! client's `clients/<name>.toml`, read, changed, and watched over
//! `rho/config/*`.
//!
//! The store reads a file again whenever its `stat` moved since the last
//! read, checked on every touch: each config request, each session open,
//! and each turn start. A file that stops loading publishes its errors and
//! leaves its last good values in force for turns and for sessions opened
//! only to read; the next touch reads it again. Nothing polls, so a client
//! sitting on a settings screen sees a hand edit on its next request.
//!
//! A session that opens to run takes the config only while the file loads
//! ([`ConfigStore::config_to_open`]), and fails otherwise, rather than run
//! on the last good values: a session opened after an edit never runs on
//! the config before it.
//!
//! A change reaches the agent when its setting says (`applies`):
//! `agent_concurrency` resizes the subagent budget as it is published, the
//! advisor and compaction settings reach each loaded session at its next
//! turn (`sessions::turn`), and the rest reach sessions opened later.

mod clients;
mod file;
mod writer_run;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{
    ConfigError as WireError, ConfigGetParams, ConfigGetResponse, ConfigLayer, ConfigNamespace,
    ConfigRegisterParams, ConfigRegisterResponse, ConfigRevision, ConfigSchema, ConfigSetParams,
    ConfigSetResponse, ConfigWatch, ErrorCode, RpcError, SettingEntry, SettingKey,
};
use tokio::sync::watch;

use self::{
    clients::Clients,
    file::{Edit, Stat},
    writer_run::WriterRun,
};
use crate::{
    app::agent_concurrency::AgentConcurrency,
    config::{
        agent_schema, agent_values, apply_agent_changes, resolve_document, AgentChangeError,
        Config, ConfigDocument, ConfigError,
    },
};

/// The daemon's settings, shared by its requests and its sessions.
pub(super) struct ConfigStore {
    agent_path: PathBuf,
    schema: ConfigSchema,
    /// Serializes reads and writes of `config.toml`.
    agent: tokio::sync::Mutex<AgentFile>,
    agent_state: watch::Sender<Arc<ConfigGetResponse>>,
    /// The last config that loaded, which turns read and sessions open to
    /// read with; `None` until one has.
    agent_config: Mutex<Option<Arc<Config>>>,
    /// Sized by `behavior.agent_concurrency` as each config is published.
    concurrency: AgentConcurrency,
    clients: Clients,
}

/// What the store knows of `config.toml` past its published state.
#[derive(Default)]
struct AgentFile {
    /// Its `stat` at the last read, `None` before the first.
    seen: Option<Stat>,
    run: WriterRun,
}

/// Why `config.toml` gave no config, with the errors of the read that
/// decided it.
#[derive(Debug, thiserror::Error)]
#[error("invalid config file {path}:\n{errors}")]
pub(super) struct ConfigUnloaded {
    path: String,
    errors: String,
}

impl ConfigStore {
    /// The settings of the home `rho_home`, whose agent config sizes
    /// `concurrency`. Reads `config.toml` once now.
    pub(super) fn new(rho_home: &Path, concurrency: AgentConcurrency) -> Self {
        let store = Self {
            agent_path: rho_home.join("config.toml"),
            schema: agent_schema(),
            agent: tokio::sync::Mutex::default(),
            agent_state: watch::Sender::new(Arc::new(ConfigGetResponse {
                revision: ConfigRevision::new(""),
                values: Default::default(),
                errors: Vec::new(),
            })),
            agent_config: Mutex::new(None),
            concurrency,
            clients: Clients::new(rho_home.join("clients")),
        };
        let stat = Stat::of(&store.agent_path);
        store.publish_agent(read_agent(&store.agent_path));
        store
            .agent
            .try_lock()
            .expect("nothing else holds a new store")
            .seen = Some(stat);
        store
    }

    /// The config a turn reads, or a session opened only to read its file:
    /// the last one that loaded, after reading `config.toml` again if it
    /// changed.
    pub(super) async fn agent_config(&self) -> Result<Arc<Config>, ConfigUnloaded> {
        self.refresh_agent(&mut self.agent.lock().await.seen);
        self.config()
            .clone()
            .ok_or_else(|| self.unloaded(&self.agent_state.borrow().errors))
    }

    /// The config a session opens to run with: `config.toml` as it is now,
    /// after reading it again if it changed. A file that does not load fails
    /// the open, naming each problem, rather than leave the session on the
    /// last good values.
    pub(super) async fn config_to_open(&self) -> Result<Arc<Config>, ConfigUnloaded> {
        let mut file = self.agent.lock().await;
        self.refresh_agent(&mut file.seen);
        let state = self.agent_state.borrow();
        match self.config().clone() {
            Some(config) if state.errors.is_empty() => Ok(config),
            _ => Err(self.unloaded(&state.errors)),
        }
    }

    /// `config.toml` as a person reads it: on Windows, without the verbatim
    /// prefix the daemon's canonical home carries.
    fn shown_path(&self) -> String {
        crate::paths::without_verbatim_prefix(self.agent_path.clone())
            .display()
            .to_string()
    }

    fn unloaded(&self, errors: &[WireError]) -> ConfigUnloaded {
        ConfigUnloaded {
            path: self.shown_path(),
            errors: listed(errors),
        }
    }

    /// The agent's `config.toml`.
    pub(super) fn agent_path(&self) -> &Path {
        &self.agent_path
    }

    pub(super) fn schema(&self) -> ConfigSchema {
        self.schema.clone()
    }

    pub(super) async fn register(
        &self,
        params: ConfigRegisterParams,
    ) -> Result<ConfigRegisterResponse, RpcError> {
        self.clients.register(params).await?;
        Ok(ConfigRegisterResponse {})
    }

    /// The namespace's state, current as of this call, to forward.
    pub(super) async fn get(&self, params: ConfigGetParams) -> Result<ConfigWatch, RpcError> {
        match params.namespace {
            ConfigNamespace::Agent => {
                self.refresh_agent(&mut self.agent.lock().await.seen);
                Ok(ConfigWatch::new(self.agent_state.subscribe()))
            }
            ConfigNamespace::Client { name } => self.clients.get(&name).await,
            _ => Err(unknown_namespace()),
        }
    }

    pub(super) async fn set(&self, params: ConfigSetParams) -> Result<ConfigSetResponse, RpcError> {
        let ConfigSetParams {
            namespace,
            layer,
            revision,
            writer,
            changes,
        } = params;
        match layer {
            ConfigLayer::Global => {}
            _ => {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    "the daemon writes only the global layer",
                ))
            }
        }
        match namespace {
            ConfigNamespace::Agent => {}
            ConfigNamespace::Client { name } => {
                return self.clients.set(&name, &revision, writer, &changes).await;
            }
            _ => return Err(unknown_namespace()),
        }
        let mut file = self.agent.lock().await;
        let path = &self.agent_path;
        // Locked before the revision check, so the file the client read is
        // the one changed.
        let mut edit = Edit::open(path)?;
        self.refresh_agent(&mut file.seen);
        let current = self.agent_state.borrow().revision.clone();
        let admitted = file.run.admit(current, &revision, writer)?;
        let (config, warnings) =
            apply_agent_changes(&mut edit.doc, &changes).map_err(|error| match error {
                AgentChangeError::Refused(errors) => refused(errors),
                AgentChangeError::FileInvalid(errors) => {
                    RpcError::new(ErrorCode::ConfigInvalid, joined(&errors))
                }
            })?;
        let (stat, doc) = edit.save()?;
        file.seen = Some(stat);
        let values = agent_values(&doc, &config);
        log_warnings(path, &warnings);
        let revision = self.publish_agent(Ok((config, values)));
        file.run.landed(admitted, revision.clone());
        Ok(ConfigSetResponse { revision })
    }

    /// Reads `config.toml` again if it changed since `seen`, the agent
    /// lock's state, and publishes what it holds.
    fn refresh_agent(&self, seen: &mut Option<Stat>) {
        let stat = Stat::of(&self.agent_path);
        if *seen == Some(stat) {
            return;
        }
        *seen = Some(stat);
        self.publish_agent(read_agent(&self.agent_path));
    }

    /// Publishes a read of `config.toml`: a config that loaded with its
    /// values, or the errors that kept it from loading, which keep the last
    /// good values. Returns the revision now in force.
    fn publish_agent(&self, read: AgentRead) -> ConfigRevision {
        let (values, errors) = match read {
            Ok((config, values)) => {
                self.concurrency.set_total(config.agent_concurrency);
                *self.config() = Some(Arc::new(config));
                (values, Vec::new())
            }
            Err(errors) => {
                // Logged once per broken edit: only a changed file is read
                // again.
                tracing::warn!(
                    path = %self.shown_path(),
                    errors = %listed(&errors),
                    "config does not load; new sessions fail to open until it is fixed"
                );
                let values = match &*self.config() {
                    Some(_) => self.agent_state.borrow().values.clone(),
                    None => agent_values(&ConfigDocument::default(), &Config::default()),
                };
                (values, errors)
            }
        };
        let state = ConfigGetResponse {
            revision: file::revision(&values),
            values,
            errors,
        };
        let revision = state.revision.clone();
        self.agent_state.send_if_modified(|current| {
            let changed = **current != state;
            if changed {
                *current = Arc::new(state);
            }
            changed
        });
        revision
    }

    /// Nothing panics while holding the lock, so a poisoned one is still
    /// consistent.
    fn config(&self) -> MutexGuard<'_, Option<Arc<Config>>> {
        self.agent_config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A loaded config with its values, or every error that kept it from
/// loading.
type AgentRead = Result<(Config, BTreeMap<SettingKey, SettingEntry>), Vec<WireError>>;

/// The config `path` holds with its values, or every error that keeps it
/// from loading.
fn read_agent(path: &Path) -> AgentRead {
    let doc = file::read(path).map_err(|error| {
        vec![WireError {
            key: None,
            message: error.message(path),
        }]
    })?;
    let (config, warnings) = resolve_document(&doc).map_err(|errors| wire_errors(&errors))?;
    log_warnings(path, &warnings);
    let values = agent_values(&doc, &config);
    Ok((config, values))
}

/// Every error of a read, one per line, by setting when it has one.
fn listed(errors: &[WireError]) -> String {
    errors
        .iter()
        .map(|error| match &error.key {
            Some(key) => format!("`{key}`: {}", error.message),
            None => error.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn log_warnings(path: &Path, warnings: &[crate::config::ConfigWarning]) {
    for warning in warnings {
        tracing::warn!(path = %path.display(), "{}", warning.message());
    }
}

pub(super) fn wire_errors(errors: &[ConfigError]) -> Vec<WireError> {
    errors
        .iter()
        .map(|error| WireError {
            key: error.key.as_deref().map(SettingKey::new),
            message: error.message.clone(),
        })
        .collect()
}

/// A change the daemon refused, naming each problem; nothing was written.
fn refused(errors: Vec<ConfigError>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, joined(&errors))
}

/// Every error, one per line.
fn joined(errors: &[ConfigError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn unknown_namespace() -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, "unknown settings namespace")
}
