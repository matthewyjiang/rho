//! `rho/auth/login` and `rho/auth/login_cancel`: interactive logins the
//! daemon runs for a client and relays to it (#1662).
//!
//! The provider's own flow runs here, the one the TUI's `/login` and ACP's
//! `authenticate` start in their processes until they become daemon clients
//! (#1667, #1670), with no browser: its prompt goes to the client that
//! asked, which opens the URL. `login/drive.rs` drives the flow, as it does
//! for the in-process logins. Once the provider approves, the daemon saves
//! the credential to the store its config selects then, so every session it
//! runs uses it from its next prompt, and refreshes their options
//! ([`super::auth::credentials_changed`]).
//!
//! One login per auth profile runs at a time, whichever client started it,
//! so two cannot race to save one profile or to bind one callback port.
//! That also bounds how many run at once by the profiles that take an
//! interactive login (seven built in: Codex, GitHub Copilot, Kimi, Meta,
//! SpaceXAI, Ollama Cloud, and OpenRouter), so no other limit is needed.
//!
//! A login ends when the provider approves or fails, a client cancels it,
//! or the connection that started it closes. Ending drops the provider's
//! flow, which closes a browser login's callback port.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{
    login_relay, AuthLoginCancelParams, AuthLoginCancelResponse, AuthLoginParams,
    AuthLoginResponse, ErrorCode, LoginFlow, LoginId, LoginOutcome, LoginRelay, LoginStream,
    RpcError,
};
use rho_providers::{
    auth::{
        browser::BrowserAvailability,
        login_dispatch::{AuthenticationError, AuthenticationMethod, ProviderAuthentication},
    },
    provider::{self, ProviderDescriptor},
};
use rho_sdk::CancellationToken;

use super::{
    auth::{credentials_changed, off_workers, repository},
    sessions::Sessions,
};
use crate::app::{
    credential_writes::{self, CredentialWrite},
    login::{
        drive::{self, Approval},
        relay_wire::{flow_from_wire, prompt_to_wire},
    },
};

/// The logins under way.
pub(super) struct Logins {
    /// By auth profile id: one login per profile at a time.
    under_way: Mutex<HashMap<&'static str, UnderWay>>,
}

/// One login under way.
struct UnderWay {
    login: LoginId,
    cancel: CancellationToken,
}

impl Logins {
    pub(super) fn new() -> Self {
        Self {
            under_way: Mutex::new(HashMap::new()),
        }
    }

    /// Starts `params`' login in a task of its own, which relays it to the
    /// stream this returns, and saves its credential before telling every
    /// one of `sessions` (see the module docs).
    pub(super) async fn start(
        self: &Arc<Self>,
        params: AuthLoginParams,
        sessions: Arc<Sessions>,
    ) -> Result<(AuthLoginResponse, LoginStream), RpcError> {
        let AuthLoginParams { auth, flow } = params;
        let flow = flow.unwrap_or_default();
        let target = off_workers(move || LoginAuth::resolve(&auth)).await??;
        let login = LoginId::new(uuid::Uuid::new_v4().to_string());
        let cancel = CancellationToken::new();
        let slot = self.claim(target.auth, &login, &cancel)?;
        let (relay, stream) = login_relay();
        tracing::info!(auth = target.auth, "login started");
        tokio::spawn(run(target, flow, cancel, relay, slot, sessions));
        Ok((AuthLoginResponse { login }, stream))
    }

    /// Asks login `params.login` to stop; its task tells its client how it
    /// ended.
    pub(super) fn cancel(
        &self,
        params: AuthLoginCancelParams,
    ) -> Result<AuthLoginCancelResponse, RpcError> {
        let under_way = self.lock();
        let Some(found) = under_way
            .values()
            .find(|under_way| under_way.login == params.login)
        else {
            return Err(RpcError::new(
                ErrorCode::LoginNotFound,
                format!("no login under way has the id {}", params.login),
            ));
        };
        found.cancel.cancel();
        Ok(AuthLoginCancelResponse {})
    }

    /// Records login `login` as `auth`'s, unless one is already under way.
    fn claim(
        self: &Arc<Self>,
        auth: &'static str,
        login: &LoginId,
        cancel: &CancellationToken,
    ) -> Result<Slot, RpcError> {
        let mut under_way = self.lock();
        if under_way.contains_key(auth) {
            return Err(RpcError::new(
                ErrorCode::LoginBusy,
                format!(
                    "the daemon is already signing in to '{auth}'; wait for that login to finish"
                ),
            ));
        }
        under_way.insert(
            auth,
            UnderWay {
                login: login.clone(),
                cancel: cancel.clone(),
            },
        );
        Ok(Slot {
            logins: Arc::clone(self),
            auth,
        })
    }

    /// Nothing panics while holding the lock, so a poisoned one is still
    /// consistent.
    fn lock(&self) -> MutexGuard<'_, HashMap<&'static str, UnderWay>> {
        self.under_way
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A login's place in [`Logins`], given up when dropped, however the
/// login's task ends, a panic included.
struct Slot {
    logins: Arc<Logins>,
    auth: &'static str,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.logins.lock().remove(self.auth);
    }
}

/// An auth profile that takes an interactive login.
struct LoginAuth {
    auth: &'static str,
    /// What the provider calls itself in a login, such as `Codex`.
    provider_label: &'static str,
    descriptor: &'static ProviderDescriptor,
}

impl LoginAuth {
    /// Checks that `auth` takes an interactive login whose credential the
    /// daemon's sessions would read. Loads the config and its store first,
    /// as every credential write does. Blocks.
    fn resolve(auth: &str) -> Result<Self, RpcError> {
        credential_writes::load_config_and_store(&repository())?;
        let invalid = |message: String| RpcError::new(ErrorCode::InvalidParams, message);
        let Some((descriptor, mode)) = provider::resolve_auth_mode(auth) else {
            return Err(invalid(format!("unknown auth profile '{auth}'")));
        };
        let method =
            ProviderAuthentication::method(mode.id).map_err(|error| invalid(error.to_string()))?;
        let provider_label = match method {
            AuthenticationMethod::Interactive { provider_label } => provider_label,
            AuthenticationMethod::ApiKey { .. } => {
                return Err(invalid(format!(
                    "'{auth}' takes an API key; send it with rho/auth/key"
                )));
            }
            AuthenticationMethod::None => {
                return Err(invalid(format!("'{auth}' needs no sign-in")));
            }
        };
        // A credential in the daemon's environment wins over a saved one,
        // so the login would change nothing.
        credential_writes::refuse_environment_override(mode.id, CredentialWrite::Login)?;
        Ok(Self {
            auth: mode.id,
            provider_label,
            descriptor,
        })
    }
}

/// How a login's wait for the provider ended.
enum Ended {
    /// The provider approved it, or why it did not.
    Approved(Result<Approval, AuthenticationError>),
    Cancelled,
    /// The connection that started it closed, so nobody can finish it.
    ClientLeft,
}

/// Runs one login to its end and tells its client how it ended. Saving is
/// not cancelled: once the provider approved, the credential is saved and
/// the login reports it.
async fn run(
    target: LoginAuth,
    flow: LoginFlow,
    cancel: CancellationToken,
    relay: LoginRelay,
    slot: Slot,
    sessions: Arc<Sessions>,
) {
    let auth = target.auth;
    // The client opens the URL, so the daemon launches no browser.
    let approved = drive::approve(
        auth,
        flow_from_wire(flow),
        BrowserAvailability::Headless,
        |prompt| relay.prompt(prompt_to_wire(prompt)),
    );
    let ended = tokio::select! {
        // An approval that is ready wins, so a login the provider approved
        // is saved even when a cancel or a disconnect lands on the same poll.
        biased;
        approved = approved => Ended::Approved(approved),
        () = cancel.cancelled() => Ended::Cancelled,
        () = relay.closed() => Ended::ClientLeft,
    };
    let outcome = match ended {
        Ended::Approved(Ok(approval)) => save(&target, approval, &sessions).await,
        Ended::Approved(Err(error)) => {
            tracing::info!(auth, "login failed");
            LoginOutcome::Failed {
                message: format!("the {} login failed: {error}", target.provider_label),
            }
        }
        Ended::Cancelled => {
            tracing::info!(auth, "login cancelled");
            LoginOutcome::Cancelled
        }
        Ended::ClientLeft => {
            tracing::info!(auth, "login cancelled: the client that started it left");
            return;
        }
    };
    // Free the profile before the client hears the end, so a login it
    // starts next is not refused as busy.
    drop(slot);
    relay.finish(outcome);
}

/// Saves what the provider approved to the store the daemon's config
/// selects now, records the login of a provider logged in through its
/// endpoint as `rho login` always has, and tells every session.
async fn save(target: &LoginAuth, approval: Approval, sessions: &Sessions) -> LoginOutcome {
    let descriptor = *target.descriptor;
    let saved = off_workers(move || -> anyhow::Result<()> {
        // The config may have chosen another store since the login started.
        credential_writes::load_config_and_store(&repository())?;
        // A provider that sends no confirmation (Ollama Cloud's device key)
        // already told the user what to approve in its prompt.
        approval.save(descriptor, &repository())
    })
    .await;
    // Even a failed save may have written part of an entry.
    credentials_changed(sessions).await;
    let auth = target.auth;
    match saved {
        Ok(Ok(())) => {
            tracing::info!(auth, "login saved");
            LoginOutcome::Succeeded
        }
        Ok(Err(error)) => {
            tracing::warn!(auth, "could not save a login: {error:#}");
            LoginOutcome::Failed {
                message: format!(
                    "could not save the {} credentials: {error:#}",
                    target.provider_label
                ),
            }
        }
        Err(error) => LoginOutcome::Failed {
            message: error.message,
        },
    }
}
