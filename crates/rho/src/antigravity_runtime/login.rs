//! `rho login antigravity`: run agy_acp_server's own Google sign-in.
//!
//! Rho spawns the server and calls ACP `authenticate` with `oauth-personal`.
//! The server prints a sign-in link to stderr, tries to open a browser, and
//! waits for Google's redirect on a loopback listener (`127.0.0.1:<port>`).
//! A browser on another machine (SSH) cannot reach that listener, so its
//! final page fails to load; pasting that page's address here replays the
//! redirect to the listener from this machine. The token never passes
//! through Rho's credential store, and the link (it carries the OAuth
//! `state`) is shown on the terminal but never written to the log.
//!
//! When the server is not installed, this first offers Rho's managed
//! install ([`super::install`]), so `/login antigravity` is the whole setup.

use std::{io::BufRead as _, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::ChildStderr,
    sync::{mpsc, watch},
};
use url::Url;

use crate::{
    acp_runtime::{self, child_transport},
    cli_runtime::{CliExecutable, OwnedChild},
};

use super::{
    executable,
    home::{AntigravityAuthStatus, AntigravityHome, PERSONAL_OAUTH_METHOD},
    install, ANTIGRAVITY_LABEL_NAME,
};

/// `_AUTH_PROMPT_MESSAGE` in agy_acp_server 1.3.0.
const LINK_PREFIX: &str = "Open the following link to authenticate the ACP server: ";
/// `_LOGIN_TIMEOUT_SECONDS` in agy_acp_server 1.3.0.
const SERVER_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// Covers server startup and `initialize` (1.4-2.1 s warm in the spike) so
/// the server's own timeout error arrives before Rho gives up.
const LOGIN_MARGIN: Duration = Duration::from_secs(10);
/// The loopback answers a pasted redirect with a static page before it
/// exchanges the code, so this bounds only a local HTTP round trip.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(10);

/// Install the server if needed, sign in, and report where the server keeps
/// the result.
pub(crate) async fn run_cli() -> anyhow::Result<()> {
    let home = AntigravityHome::from_env(&crate::paths::home_dir().unwrap_or_default());
    let server = match executable::resolve() {
        Ok(server) => server,
        Err(executable::AntigravityExecutableError::BinaryMissing) => {
            CliExecutable::from_path(install::offer_install().await?)
        }
    };
    // Private (0600) and uniquely named; kept only when sign-in fails.
    let log = tempfile::Builder::new()
        .prefix("rho-antigravity-login-")
        .suffix(".log")
        .tempfile()?;
    let log_file = tokio::fs::File::from_std(log.reopen()?);
    let mut command = server.try_command(executable::server_args())?;
    command
        .envs(executable::harness_env_for_launch(/*frozen*/ None))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = OwnedChild::spawn(command).map_err(|source| {
        anyhow::anyhow!(
            "{ANTIGRAVITY_LABEL_NAME}: failed to spawn `{}`: {source}",
            server.display()
        )
    })?;
    let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin(), child.stdout(), child.stderr())
    else {
        anyhow::bail!("{ANTIGRAVITY_LABEL_NAME}: server pipes are unavailable");
    };
    eprintln!("{ANTIGRAVITY_LABEL_NAME} login");
    eprintln!("starting {} ...", executable::ANTIGRAVITY_PROGRAM);
    let (redirect_tx, redirect_rx) = watch::channel(None);
    let stderr_task = tokio::spawn(watch_server_stderr(stderr, log_file, redirect_tx));
    let paste_task = tokio::spawn(replay_pasted_redirects(redirect_rx));
    let signed_in = acp_runtime::login::authenticate(
        child_transport(stdin, stdout, ANTIGRAVITY_LABEL_NAME),
        PERSONAL_OAUTH_METHOD,
        SERVER_LOGIN_TIMEOUT.saturating_add(LOGIN_MARGIN),
    );
    // A descendant can inherit stdout, so also watch the leader exit.
    let outcome = tokio::select! {
        outcome = signed_in => outcome,
        status = child.wait() => Err(match status {
            Ok(status) => format!("the server exited before sign-in finished ({status})"),
            Err(error) => format!("could not wait for the server: {error}"),
        }),
    };
    paste_task.abort();
    child.terminate().await;
    let _ = stderr_task.await;
    let status = home.status();
    let failure = match (&outcome, &status) {
        (Err(error), _) => format!("could not sign in to {ANTIGRAVITY_LABEL_NAME}: {error}"),
        (Ok(()), AntigravityAuthStatus::Configured { method }) => {
            // Dropping the temp file deletes the log; it only explains failures.
            eprintln!(
                "signed in to {ANTIGRAVITY_LABEL_NAME} ({method}); the server keeps the token in {}",
                crate::paths::display(home.acp_dir())
            );
            return Ok(());
        }
        (
            Ok(()),
            AntigravityAuthStatus::SignedOut { .. }
            | AntigravityAuthStatus::MissingToken { .. }
            | AntigravityAuthStatus::Unreadable { .. },
        ) => {
            let detail = status.require_signed_in().err().unwrap_or_default();
            format!("sign-in reported success but {detail}")
        }
    };
    match log.keep() {
        Ok((_, path)) => anyhow::bail!("{failure} (server log: {})", crate::paths::display(&path)),
        Err(_) => anyhow::bail!("{failure}"),
    }
}

/// Log server diagnostics; print the sign-in link (never logged) and publish
/// its loopback redirect endpoint.
async fn watch_server_stderr(
    stderr: ChildStderr,
    mut log: tokio::fs::File,
    redirect_tx: watch::Sender<Option<Url>>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Some(link) = printed_link(&line) else {
            let _ = log.write_all(line.as_bytes()).await;
            let _ = log.write_all(b"\n").await;
            continue;
        };
        let _ = log.write_all(b"[sign-in link omitted]\n").await;
        let redirect = loopback_redirect(&link);
        eprintln!("\nOpen this link in a browser and sign in:\n\n{link}\n");
        match &redirect {
            Some(redirect) => eprintln!(
                "If the browser runs on another machine, it ends on a page that cannot load \
({redirect}...). Paste that page's address here and press Enter.\n\
Waiting up to {} s.",
                SERVER_LOGIN_TIMEOUT.as_secs()
            ),
            None => eprintln!("Waiting up to {} s.", SERVER_LOGIN_TIMEOUT.as_secs()),
        }
        let _ = redirect_tx.send(redirect);
    }
}

/// Replay pasted redirect addresses to the server's loopback listener.
async fn replay_pasted_redirects(redirect_rx: watch::Receiver<Option<Url>>) {
    // Never follow a `Location`: the validated loopback request is the only
    // one Rho sends.
    let client = match crate::reqwest_client_builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REPLAY_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("pasting the redirect is unavailable: {error}");
            return;
        }
    };
    let mut lines = stdin_lines();
    while let Some(line) = lines.recv().await {
        if line.trim().is_empty() {
            continue;
        }
        let Some(redirect) = redirect_rx.borrow().clone() else {
            eprintln!("the sign-in link has not appeared yet; paste the address after it does");
            continue;
        };
        let target = match local_redirect(&line, &redirect) {
            Ok(target) => target,
            Err(error) => {
                eprintln!("{error}");
                continue;
            }
        };
        match client.get(target).send().await {
            Ok(response) if response.status().is_success() => {
                eprintln!("sent the redirect; waiting for the server to finish sign-in");
            }
            Ok(response) => eprintln!(
                "the server answered the redirect with {}",
                response.status()
            ),
            // The URL carries the authorization code; keep it off the terminal.
            Err(error) => eprintln!(
                "could not reach the server's listener: {}",
                error.without_url()
            ),
        }
    }
}

/// Terminal lines from a detached thread. `tokio::io::stdin` reads on the
/// blocking pool, which runtime shutdown waits for, so an unanswered read
/// would hold the process open after sign-in ends.
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The sign-in URL from one server stderr line.
fn printed_link(line: &str) -> Option<Url> {
    let (_, rest) = line.split_once(LINK_PREFIX)?;
    Url::parse(rest.trim()).ok()
}

/// The link's `redirect_uri` when it is a loopback listener with a port.
fn loopback_redirect(link: &Url) -> Option<Url> {
    let (_, redirect) = link
        .query_pairs()
        .find(|(name, _)| name == "redirect_uri")?;
    let redirect = Url::parse(&redirect).ok()?;
    (redirect.scheme() == "http"
        && redirect.host_str() == Some("127.0.0.1")
        && redirect.port().is_some()
        && redirect.query().is_none())
    .then_some(redirect)
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
enum PasteError {
    #[error("that is not a URL; paste the full address of the page the browser ended on")]
    NotAUrl,
    #[error("that address is not the sign-in redirect; expected {expected}?code=...")]
    NotTheRedirect { expected: Url },
}

/// The request to replay: the advertised `redirect` endpoint carrying the
/// pasted query. Only that exact listener (scheme, port, path; `localhost`
/// allowed for the host) is accepted, so pasted text can never make Rho send
/// a request anywhere else.
fn local_redirect(pasted: &str, redirect: &Url) -> Result<Url, PasteError> {
    let url = Url::parse(pasted.trim()).map_err(|_| PasteError::NotAUrl)?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    let answers_flow = url
        .query_pairs()
        .any(|(name, _)| name == "code" || name == "error");
    let same_listener = url.scheme() == redirect.scheme()
        && url.port() == redirect.port()
        && url.path() == redirect.path()
        && url.username().is_empty()
        && url.password().is_none();
    if !(loopback && same_listener && answers_flow) {
        return Err(PasteError::NotTheRedirect {
            expected: redirect.clone(),
        });
    }
    let mut target = redirect.clone();
    target.set_query(url.query());
    Ok(target)
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
