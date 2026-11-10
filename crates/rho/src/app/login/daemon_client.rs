//! `rho login` as a client of the rho daemon (#1662), which it starts when
//! none runs, as `rho run --detach` does.
//!
//! The daemon checks and saves an API key (`rho/auth/key`), or runs the
//! provider's interactive login (`rho/auth/login`) and relays its prompt
//! here; this process opens the browser, prints the prompt as the
//! in-process login does, and waits for the login to finish. Either way
//! the credential lands in the store the daemon's config selects, and every
//! session the daemon runs uses it from its next prompt.
//!
//! A credential variable set in this shell would override what the daemon
//! saves, in every rho started here, so `rho login` refuses before asking.
//! The requests carry this shell's environment, so the daemon saves to the
//! store a `RHO_CREDENTIAL_STORE` here chooses, the one sessions opened from
//! here read (#1779).
//!
//! When neither this shell, the daemon's environment, nor its config chose
//! a credential store, `rho login` asks which to use first, offering what the daemon can
//! reach, since the daemon writes the credential: one a client started from
//! an SSH login may have no session D-Bus for the OS store even where this
//! shell has one. The daemon checks the store takes a secret and saves the
//! choice (`rho/auth/choose_store`).
//!
//! Interrupting `rho login` ends its connection, and the daemon cancels an
//! interactive login whose client left, unless the provider already
//! approved it. A key the daemon already received is saved.

use anyhow::anyhow;
use rho_protocol::{
    methods::{AuthChooseStore, AuthKey, AuthLogin, Status},
    AuthChooseStoreParams, AuthKeyParams, AuthLoginParams, Client, ClientEnvironment, ClientError,
    DaemonNotification, ErrorCode, LoginOutcome, StatusParams, StoreSelectedBy,
};
use rho_providers::auth::browser::{self, BrowserAvailability};

use super::{
    drive,
    relay_wire::{flow_to_wire, prompt_from_wire},
    secret,
    store_choice::{self, StoreOffers},
    SignIn, Target,
};
use crate::{
    app::{credential_writes, daemon_cli},
    credential_store,
    daemon::Connection,
    login_prompt_print::eprint_login_prompt,
};

/// Signs `target` in through the daemon, with a device-code login when
/// `device_auth` asks for one.
pub(super) async fn run(target: Target, device_auth: bool) -> anyhow::Result<()> {
    let Target { auth, sign_in, .. } = target;
    refuse_shell_override(&auth, |variable| std::env::var(variable).ok())?;
    match sign_in {
        SignIn::ApiKey { entry_label } => save_key(auth, entry_label).await,
        SignIn::Interactive { provider_label } => log_in(auth, provider_label, device_auth).await,
    }
}

/// Refuses `auth` when `environment`, this shell's, holds its credential:
/// the variable wins over any saved credential in every rho started here,
/// a daemon `rho login` starts included, so saving one would change
/// nothing until it is unset.
fn refuse_shell_override(
    auth: &str,
    environment: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<()> {
    match credential_writes::environment_override(auth, environment) {
        Some(variable) => anyhow::bail!(
            "{variable} is set in this shell and overrides any credential saved for '{auth}', here and in a rho daemon started from here; unset it, then run rho login again"
        ),
        None => Ok(()),
    }
}

/// Reads the key before reaching the daemon, so a slow start does not hold
/// up the prompt.
async fn save_key(auth: String, entry_label: &str) -> anyhow::Result<()> {
    let key = secret::read(entry_label).await?;
    let Connection { client, .. } = daemon_cli::connect().await?;
    ensure_store(&client).await?;
    let saved = client
        .request::<AuthKey>(&AuthKeyParams {
            auth: auth.clone(),
            key,
            environment: ClientEnvironment::of_this_process(),
        })
        .await
        .map_err(|error| refused(error, format!("could not save credentials for {auth}")))?;
    match saved.unchecked {
        None => eprintln!("saved credentials for {auth}"),
        Some(unchecked) => eprintln!(
            "saved credentials for {auth} without checking them: {}",
            unchecked.message
        ),
    }
    Ok(())
}

async fn log_in(auth: String, provider_label: &str, device_auth: bool) -> anyhow::Result<()> {
    // The browser opens here, so this process's display decides.
    let availability = BrowserAvailability::from_process();
    let flow = flow_to_wire(drive::mode(&auth, device_auth, availability));
    let Connection {
        client,
        mut notifications,
        ..
    } = daemon_cli::connect().await?;
    ensure_store(&client).await?;
    let login = client
        .request::<AuthLogin>(&AuthLoginParams {
            auth: auth.clone(),
            flow: Some(flow),
            environment: ClientEnvironment::of_this_process(),
        })
        .await
        .map_err(|error| refused(error, format!("could not start the {provider_label} login")))?
        .login;
    loop {
        let Some(notification) = notifications.recv().await else {
            anyhow::bail!(
                "lost the connection to the rho daemon during the {provider_label} login"
            );
        };
        match notification {
            DaemonNotification::LoginPrompt(params) if params.login == login => {
                let prompt = prompt_from_wire(params.prompt);
                let opened = browser::try_open(prompt.copyable_url(), availability);
                eprint_login_prompt(provider_label, &prompt.with_browser(opened));
            }
            DaemonNotification::LoginFinished(params) if params.login == login => {
                return finished(&auth, params.outcome);
            }
            // Nothing else is sent on this connection; the daemon may add
            // notifications.
            _ => {}
        }
    }
}

/// Asks which credential store to use when nothing chose one for the
/// daemon, offering the OS store as the daemon reaches it, and has the
/// daemon keep credentials in the answer. A `RHO_CREDENTIAL_STORE` in this
/// shell already chose the store of what is signed in from here, so nothing
/// is asked; a value that names no store fails here, as it would there.
async fn ensure_store(client: &Client) -> anyhow::Result<()> {
    if credential_store::environment_backend()?.is_some() {
        return Ok(());
    }
    let status = client
        .request::<Status>(&StatusParams {})
        .await
        .map_err(|error| {
            refused(
                error,
                "could not ask the rho daemon about its credential store".into(),
            )
        })?
        .credential_store;
    if status.selected_by != StoreSelectedBy::Default {
        return Ok(());
    }
    let offers = StoreOffers {
        os_unavailable: status.unreachable,
        // The daemon checks the file store when it is chosen.
        file_unavailable: None,
    };
    let unattended = "the rho daemon's credential store is unset; set it before non-interactive login with `rho credential-store set file` or behavior.credential_store in config.toml, which the daemon reads, or set RHO_CREDENTIAL_STORE=os|file in this shell";
    let backend =
        tokio::task::spawn_blocking(move || store_choice::ask(&offers, unattended)).await??;
    client
        .request::<AuthChooseStore>(&AuthChooseStoreParams {
            store: credential_store::wire::kind(backend),
            environment: ClientEnvironment::of_this_process(),
        })
        .await
        .map_err(|error| {
            refused(
                error,
                format!("could not choose the {} credential store", backend.as_str()),
            )
        })?;
    eprintln!(
        "credential store set to {} in the rho daemon's config",
        backend.as_str()
    );
    Ok(())
}

/// What `rho login` reports for a login to `auth` that ended with
/// `outcome`.
fn finished(auth: &str, outcome: LoginOutcome) -> anyhow::Result<()> {
    match outcome {
        // An Ollama Cloud device key saves before the user approves it, and
        // the prompt said to approve it, so this claims the save, not the
        // approval.
        LoginOutcome::Succeeded => {
            eprintln!("Saved the {auth} credential");
            Ok(())
        }
        LoginOutcome::Failed { message } => anyhow::bail!("{message}"),
        LoginOutcome::Cancelled => anyhow::bail!("the login to {auth} was cancelled"),
        // `Unknown`, or an outcome a newer daemon added.
        _ => anyhow::bail!(
            "the login to {auth} ended in a way this rho does not know; is the daemon newer than this rho?"
        ),
    }
}

/// `error`, the daemon's refusal of what `doing` describes; a daemon too
/// old to know the method gets told how to replace it.
fn refused(error: ClientError, doing: String) -> anyhow::Error {
    match error.error_code() {
        Some(ErrorCode::MethodNotFound) => anyhow!(
            "{doing}: the running rho daemon is older than this rho and cannot sign in; stop it with `rho daemon stop`, then run rho login again to start this rho's daemon"
        ),
        _ => anyhow::Error::new(error).context(doing),
    }
}

#[cfg(test)]
#[path = "daemon_client_tests.rs"]
mod tests;
