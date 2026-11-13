//! Sign-in for ACP hosts, run by the daemon (`rho/auth/*`).
//!
//! The daemon lists the auth profiles and runs their logins, saving the
//! credential where every rho reads it. `initialize` advertises its
//! interactive logins as [`AuthMethod::Agent`] to hosts that open URLs:
//! `authenticate` starts the login in the daemon and shows its link as a
//! URL-mode elicitation. Every other login, API keys included (a secret
//! must never travel in an ACP message: hosts log them), is
//! [`AuthMethod::Terminal`], which runs `rho acp --login <auth>` in the
//! host's terminal, for hosts that have one.

use std::{
    future::Future,
    sync::{Arc, Mutex, PoisonError},
};

use agent_client_protocol::{
    v1::schema::{
        AuthMethod, AuthMethodAgent, AuthMethodTerminal, AuthenticateResponse,
        CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse,
        ElicitationAction, ElicitationId, ElicitationRequestScope, ElicitationUrlMode,
        InitializeRequest, LogoutResponse, RequestId,
    },
    Error as AcpError, RequestCancellation,
};
use rho_protocol::{
    methods::{AuthList, AuthLogin, AuthLoginCancel, AuthLogout},
    AuthKind, AuthListParams, AuthLoginCancelParams, AuthLoginParams, AuthLogoutParams,
    AuthSummary, ClientEnvironment, LoginId, LoginOutcome,
};
use rho_providers::auth::{browser::BrowserAvailability, login_prompt::LoginPrompt};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

use super::{
    daemon,
    host_features::{HostFeatures, Turns},
    AcpClientPort, Adapter,
};
use crate::{
    agent::{AgentCatalog, DEFAULT_AGENT_ID},
    app::{
        agent_binding::{root_host_tools, AgentBinder, AgentInvocation, AgentRole, LaunchTools},
        config_repository::ConfigRepository,
        daemon_client::{Daemon, LoginEvent},
        login::{self, drive, relay_wire},
    },
};

/// The auth methods `initialize` advertises for `auths` to a host with
/// `features`: logins in the host where it opens URLs, else in its
/// terminal, if it has one. A sign-in whose approval nothing reports is
/// left to `rho login`: a host takes a finished `authenticate` or a zero
/// `--login` exit as signed in.
pub(super) fn methods(auths: &[AuthSummary], features: HostFeatures) -> Vec<AuthMethod> {
    auths
        .iter()
        .filter(|summary| login::confirms_sign_in(&summary.auth))
        .filter_map(|summary| {
            let id = summary.auth.clone();
            let label = summary.label.clone();
            match summary.kind {
                AuthKind::Login if features.urls => {
                    Some(AuthMethod::Agent(AuthMethodAgent::new(id, label)))
                }
                AuthKind::Login | AuthKind::ApiKey if features.terminal => {
                    Some(AuthMethod::Terminal(
                        AuthMethodTerminal::new(id, label)
                            .args(vec!["--login".into(), summary.auth.clone()]),
                    ))
                }
                AuthKind::Login | AuthKind::ApiKey => None,
                // `Unknown`, or a kind a newer daemon sends: nothing this
                // rho can sign in to.
                _ => None,
            }
        })
        .collect()
}

/// The auth profile a new session of agent `agent` (`--agent`, else the
/// default agent) uses, bound as the daemon binds it: the agent's own when
/// it pins one, else the one the config selects. The agent is found from
/// this process's directory, where the daemon looks in the session's `cwd`;
/// hosts start `rho acp` in the workspace they open sessions in.
fn session_auth(agent: Option<&str>) -> anyhow::Result<String> {
    let config = ConfigRepository::new(None).load()?;
    config.providers.activate()?;
    let catalog = AgentCatalog::discover(
        &std::env::current_dir()?,
        crate::workspace::ProjectTrusts::of_this_process(),
    )?;
    let definition = Arc::new(
        catalog
            .find(agent.unwrap_or(DEFAULT_AGENT_ID))?
            .definition
            .clone(),
    );
    let bound = AgentBinder::bind(
        definition,
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools: root_host_tools(&config, LaunchTools::All),
        },
        &config,
    )?;
    Ok(bound.rho_config().unwrap_or(&config).auth.clone())
}

impl Adapter {
    /// The auth methods for the host that sent `request`. A daemon that
    /// cannot be reached advertises none; its sessions fail the same way.
    pub(super) async fn auth_methods(&self, request: &InitializeRequest) -> Vec<AuthMethod> {
        match self
            .daemon
            .request::<AuthList>(&AuthListParams {
                environment: ClientEnvironment::of_this_process(),
            })
            .await
        {
            // Which turns the host follows has no bearing on how it signs in.
            Ok(list) => methods(
                &list.auths,
                HostFeatures::from_initialize(request, Turns::default()),
            ),
            Err(error) => {
                tracing::warn!(%error, "could not list the rho daemon's auth profiles");
                Vec::new()
            }
        }
    }

    /// `authenticate`: signs in to `auth` through the daemon, showing the
    /// provider's link in the host. `request_id` scopes the elicitation; a
    /// host that cancels the request through `cancellation` cancels the
    /// login, which has ended when the reply goes out.
    pub(super) async fn authenticate(
        &self,
        auth: &str,
        request_id: RequestId,
        port: &dyn AcpClientPort,
        cancellation: RequestCancellation,
    ) -> Result<AuthenticateResponse, AcpError> {
        let login = self.login(auth, request_id, port, &cancellation);
        self.logins.run(login).await?;
        Ok(AuthenticateResponse::new())
    }

    /// `logout`: signs out of the auth profile new sessions of this host
    /// use (see [`session_auth`]).
    pub(super) async fn logout(&self) -> Result<LogoutResponse, AcpError> {
        self.logins
            .run(async {
                let auth = session_auth(self.launch.agent.as_deref()).map_err(|error| {
                    AcpError::internal_error().data(format!(
                        "could not find the auth profile to sign out of: {error:#}"
                    ))
                })?;
                if auth.is_empty() {
                    return Err(AcpError::invalid_request().data("no auth profile is selected"));
                }
                self.daemon
                    .request::<AuthLogout>(&AuthLogoutParams {
                        auth,
                        environment: ClientEnvironment::of_this_process(),
                    })
                    .await?;
                Ok(LogoutResponse::new())
            })
            .await
    }

    async fn login(
        &self,
        auth: &str,
        request_id: RequestId,
        port: &dyn AcpClientPort,
        cancellation: &RequestCancellation,
    ) -> Result<(), AcpError> {
        if !self.negotiated.get().urls {
            return Err(AcpError::invalid_params().data(format!(
                "this host cannot open sign-in links; run `rho login {auth}` in a terminal"
            )));
        }
        let label = self.auth_label(auth).await;
        let link = self.daemon.link().await?;
        let (sender, events) = mpsc::unbounded_channel();
        self.daemon.route_login(sender);
        let mut running = RunningLogin {
            daemon: &self.daemon,
            settling: &self.logins.settling,
            events,
            login: None,
        };
        let mode = drive::mode(
            auth,
            /*device_auth*/ false,
            BrowserAvailability::from_process(),
        );
        let params = AuthLoginParams {
            auth: auth.to_owned(),
            flow: Some(relay_wire::flow_to_wire(mode)),
            environment: ClientEnvironment::of_this_process(),
        };
        let started = link
            .request::<AuthLogin>(&params)
            .await
            .map_err(|error| daemon::daemon_error(&error))?;
        running.login = Some(started.login);
        let prompt = match running.next(Some(cancellation)).await? {
            Step::Prompt(prompt) => relay_wire::prompt_from_wire(prompt),
            Step::Finished(outcome) => return finished(outcome),
            Step::Cancelled => return Err(running.cancel().await),
        };
        let elicitation_id = ElicitationId::new(uuid::Uuid::new_v4().to_string());
        let mut shown = port.create_elicitation(CreateElicitationRequest::new(
            ElicitationUrlMode::new(
                ElicitationRequestScope::new(request_id),
                elicitation_id.clone(),
                prompt.copyable_url(),
            ),
            message(&label, &prompt),
        ));
        // The host answers once the user chooses to open the URL, which may
        // be after the provider already finished (a code approved on a
        // phone).
        let raced = tokio::select! {
            ended = running.ending(Some(cancellation)) => Raced::Ended(ended?),
            answer = &mut shown => Raced::Answered(answer),
        };
        let ended = match raced {
            Raced::Ended(ended) => ended,
            Raced::Answered(answer) => {
                match answer
                    .map_err(|error| unshown(&label, auth, &error))?
                    .action
                {
                    ElicitationAction::Accept(_) => running.ending(Some(cancellation)).await?,
                    ElicitationAction::Decline | ElicitationAction::Cancel => None,
                    // An action this version does not know is not consent
                    // to wait.
                    _ => None,
                }
            }
        };
        let Some(outcome) = ended else {
            return Err(running.cancel().await);
        };
        finished(outcome)?;
        if let Err(error) = port
            .complete_elicitation(CompleteElicitationNotification::new(elicitation_id))
            .await
        {
            tracing::warn!(%error, "could not mark the sign-in elicitation complete");
        }
        // If the host never answered, dropping the request now withdraws it
        // with `$/cancel_request`; `elicitation/complete` went first, so the
        // host already knows the sign-in finished.
        drop(shown);
        Ok(())
    }

    /// How the daemon names `auth`, or the id itself.
    async fn auth_label(&self, auth: &str) -> String {
        let list = self
            .daemon
            .request::<AuthList>(&AuthListParams {
                environment: ClientEnvironment::of_this_process(),
            })
            .await;
        list.ok()
            .and_then(|list| list.auths.into_iter().find(|summary| summary.auth == auth))
            .map_or_else(|| auth.to_owned(), |summary| summary.label)
    }
}

/// What came first once the link is shown.
enum Raced {
    /// The login ended, or the host cancelled (`None`).
    Ended(Option<LoginOutcome>),
    Answered(Result<CreateElicitationResponse, AcpError>),
}

/// What a login reported next.
enum Step {
    Prompt(rho_protocol::LoginPrompt),
    Finished(LoginOutcome),
    /// The host cancelled `authenticate`.
    Cancelled,
}

fn finished(outcome: LoginOutcome) -> Result<(), AcpError> {
    match outcome {
        LoginOutcome::Succeeded => Ok(()),
        LoginOutcome::Failed { message } => Err(AcpError::internal_error().data(message)),
        LoginOutcome::Cancelled => Err(AcpError::request_cancelled()),
        // `Unknown`, or an outcome a newer daemon sends.
        _ => {
            Err(AcpError::internal_error()
                .data("the sign-in ended in a way this rho does not know"))
        }
    }
}

fn message(label: &str, prompt: &LoginPrompt) -> String {
    match &prompt.user_code {
        Some(code) => format!("Sign in to {label} with code {code}."),
        None => format!("Sign in to {label} in your browser."),
    }
}

/// The host refused to show the sign-in link, so the login cannot finish
/// here; the terminal login still can.
fn unshown(label: &str, auth: &str, error: &AcpError) -> AcpError {
    AcpError::internal_error().data(format!(
        "could not show the {label} sign-in link: {error}; run `rho login {auth}` instead"
    ))
}

/// The login route, and the daemon's login while it may still run.
/// Dropping it while the login runs, as when a newer login or logout
/// supersedes it, cancels the login; the next one waits until it ended
/// ([`LoginFlight`]), so a login's callback listener is gone before the
/// next login binds one.
struct RunningLogin<'a> {
    daemon: &'a Arc<Daemon>,
    settling: &'a Mutex<Option<JoinHandle<()>>>,
    events: mpsc::UnboundedReceiver<LoginEvent>,
    login: Option<LoginId>,
}

impl RunningLogin<'_> {
    /// The login's next step. `cancellation` is the host's cancel of
    /// `authenticate`; `None` once the login is being cancelled anyway.
    async fn next(&mut self, cancellation: Option<&RequestCancellation>) -> Result<Step, AcpError> {
        let cancelled = async {
            match cancellation {
                Some(cancellation) => cancellation.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let mut cancelled = std::pin::pin!(cancelled);
        loop {
            let event = tokio::select! {
                event = self.events.recv() => event,
                () = &mut cancelled => return Ok(Step::Cancelled),
            };
            let ours = |id: &LoginId| self.login.as_ref() == Some(id);
            match event {
                Some(LoginEvent::Prompt { login, prompt }) if ours(&login) => {
                    return Ok(Step::Prompt(prompt));
                }
                Some(LoginEvent::Finished { login, outcome }) if ours(&login) => {
                    self.login = None;
                    return Ok(Step::Finished(outcome));
                }
                // An earlier login's last word.
                Some(LoginEvent::Prompt { .. } | LoginEvent::Finished { .. }) => {}
                Some(LoginEvent::Lost) | None => {
                    self.login = None;
                    return Err(daemon::lost());
                }
            }
        }
    }

    /// How the login ends, or `None` when the host cancelled first; a
    /// repeated prompt changes nothing shown.
    async fn ending(
        &mut self,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<Option<LoginOutcome>, AcpError> {
        loop {
            match self.next(cancellation).await? {
                Step::Prompt(_) => {}
                Step::Finished(outcome) => return Ok(Some(outcome)),
                Step::Cancelled => return Ok(None),
            }
        }
    }

    /// Cancels the login and waits until it ended; returns the error the
    /// abandoned `authenticate` answers.
    async fn cancel(&mut self) -> AcpError {
        if let Some(login) = self.login.clone() {
            if cancel(self.daemon, login).await {
                let _ = self.ending(/*cancellation*/ None).await;
            }
        }
        self.login = None;
        AcpError::request_cancelled()
    }
}

impl Drop for RunningLogin<'_> {
    fn drop(&mut self) {
        let Some(login) = self.login.take() else {
            self.daemon.unroute_login();
            return;
        };
        let daemon = Arc::clone(self.daemon);
        let mut events = std::mem::replace(&mut self.events, mpsc::unbounded_channel().1);
        let settled = tokio::spawn(async move {
            if cancel(&daemon, login.clone()).await {
                while let Some(event) = events.recv().await {
                    match event {
                        LoginEvent::Finished { login: id, .. } if id == login => break,
                        LoginEvent::Prompt { .. } | LoginEvent::Finished { .. } => {}
                        LoginEvent::Lost => break,
                    }
                }
            }
            daemon.unroute_login();
        });
        *self.settling.lock().unwrap_or_else(PoisonError::into_inner) = Some(settled);
    }
}

/// Asks the daemon to stop `login`; whether it was still under way.
async fn cancel(daemon: &Daemon, login: LoginId) -> bool {
    let params = AuthLoginCancelParams { login };
    match daemon.request::<AuthLoginCancel>(&params).await {
        Ok(_) => true,
        Err(error) => {
            tracing::debug!(%error, "could not cancel a sign-in");
            false
        }
    }
}

/// One login or logout at a time per connection. A new `authenticate` or
/// `logout` supersedes every earlier one, running or still queued, and starts
/// only after the running one is dropped, so the daemon runs one login for
/// this host and no login finishes after a later logout. Hosts may leave a
/// sign-in without sending `$/cancel_request`, which would otherwise hold
/// the login until the provider times out.
pub(super) struct LoginFlight {
    running: tokio::sync::Mutex<()>,
    /// The generation of the newest `run`; older runs end when it moves.
    latest: watch::Sender<u64>,
    /// The cancel of a superseded login, which the next run waits out.
    settling: Mutex<Option<JoinHandle<()>>>,
}

impl Default for LoginFlight {
    fn default() -> Self {
        Self {
            running: tokio::sync::Mutex::new(()),
            latest: watch::channel(0).0,
            settling: Mutex::new(None),
        }
    }
}

impl LoginFlight {
    pub(super) async fn run<T>(
        &self,
        login: impl Future<Output = Result<T, AcpError>>,
    ) -> Result<T, AcpError> {
        let mut generation = 0;
        self.latest.send_modify(|latest| {
            *latest += 1;
            generation = *latest;
        });
        let mut newer = self.latest.subscribe();
        let mut superseded = std::pin::pin!(async move {
            // A closed channel cannot happen while `self` lives; treat it
            // as superseded rather than wait forever.
            let _ = newer.wait_for(|latest| *latest != generation).await;
        });
        let running = tokio::select! {
            running = self.running.lock() => running,
            () = &mut superseded => return Err(AcpError::request_cancelled()),
        };
        let settling = self
            .settling
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(settling) = settling {
            let _ = settling.await;
        }
        // `select!` drops the losing login before this statement ends, so the
        // next run cannot start while this login still runs.
        let result = tokio::select! {
            result = login => result,
            () = &mut superseded => Err(AcpError::request_cancelled()),
        };
        drop(running);
        result
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
