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
//! Interrupting `rho login` ends its connection, and the daemon cancels a
//! login whose client left.

use anyhow::Context;
use rho_protocol::{
    methods::{AuthKey, AuthLogin},
    AuthKeyParams, AuthLoginParams, DaemonNotification, LoginFlow, LoginOutcome,
};
use rho_providers::auth::{
    browser::{self, BrowserAvailability},
    login_dispatch::{InteractiveLoginMode, ProviderAuthentication},
    login_prompt::LoginPrompt,
};

use super::{secret, SignIn, Target};
use crate::{
    app::daemon_cli,
    daemon::{self, cli_client_info, Connection, ProcessSpawner},
    login_prompt_print::eprint_login_prompt,
};

/// Signs `target` in through the daemon, with a device-code login when
/// `device_auth` asks for one.
pub(super) async fn run(target: Target, device_auth: bool) -> anyhow::Result<()> {
    let Target { auth, sign_in, .. } = target;
    match sign_in {
        SignIn::ApiKey { entry_label } => save_key(auth, entry_label).await,
        SignIn::Interactive { provider_label } => log_in(auth, provider_label, device_auth).await,
    }
}

/// Reads the key before reaching the daemon, so a slow start does not hold
/// up the prompt.
async fn save_key(auth: String, entry_label: &str) -> anyhow::Result<()> {
    let key = secret::read(entry_label).await?;
    let Connection { client, .. } = connect().await?;
    let saved = client
        .request::<AuthKey>(&AuthKeyParams {
            auth: auth.clone(),
            key,
        })
        .await
        .with_context(|| format!("could not save credentials for {auth}"))?;
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
    let mode = if device_auth {
        InteractiveLoginMode::Device
    } else {
        ProviderAuthentication::preferred_mode(&auth, availability)
    };
    let flow = match mode {
        InteractiveLoginMode::Browser => LoginFlow::Browser,
        InteractiveLoginMode::Device => LoginFlow::DeviceCode,
    };
    let Connection {
        client,
        mut notifications,
        ..
    } = connect().await?;
    let login = client
        .request::<AuthLogin>(&AuthLoginParams {
            auth: auth.clone(),
            flow: Some(flow),
        })
        .await
        .with_context(|| format!("could not start the {provider_label} login"))?
        .login;
    loop {
        let Some(notification) = notifications.recv().await else {
            anyhow::bail!(
                "lost the connection to the rho daemon during the {provider_label} login"
            );
        };
        match notification {
            DaemonNotification::LoginPrompt(params) if params.login == login => {
                let prompt = from_wire(params.prompt);
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

/// What `rho login` reports for a login to `auth` that ended with
/// `outcome`.
fn finished(auth: &str, outcome: LoginOutcome) -> anyhow::Result<()> {
    match outcome {
        LoginOutcome::Succeeded => {
            eprintln!("Successfully logged in to {auth}");
            Ok(())
        }
        // A person reads the instructions; `rho acp --login`, whose exit
        // status promises a finished sign-in, signs in in its own process.
        LoginOutcome::Unconfirmed { instruction } => {
            eprintln!("{instruction}");
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

/// `prompt` as the in-process login shows it, before the browser opens.
fn from_wire(prompt: rho_protocol::LoginPrompt) -> LoginPrompt {
    let rho_protocol::LoginPrompt {
        url,
        user_code,
        url_with_code,
        instruction,
    } = prompt;
    match user_code {
        Some(user_code) => LoginPrompt::device_code(url, user_code, url_with_code, instruction),
        None => LoginPrompt::browser_flow(url, instruction),
    }
}

async fn connect() -> anyhow::Result<Connection> {
    let paths = daemon_cli::paths()?;
    let spawner = ProcessSpawner::current_exe().context("could not find the rho executable")?;
    daemon::connect_or_spawn(&paths, cli_client_info(), &spawner)
        .await
        .context("could not reach the rho daemon")
}
