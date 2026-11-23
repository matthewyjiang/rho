//! Reading pull requests through GitHub's CLI, `gh`, on the daemon's
//! machine: finding it, running one GraphQL query under [`DEADLINE`], and
//! telling why a read failed.

use std::{
    ffi::{OsStr, OsString},
    future::Future,
    io,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use tokio::process::Command;

use super::{
    gh_query,
    model::{Batch, FetchPlan},
};
use crate::executable::{find_on, is_executable_file};

/// Reads pull requests from a forge host.
///
/// The PR service's poller is the only caller, and it makes one read at a
/// time, so implementors need not queue. `GhCli` is the one production
/// implementor; tests script answers. An implementor answers what it could
/// read, with parts it couldn't (a missing repository, a PR number with no
/// PR) inside the [`Batch`], and returns [`ReadError`] only when nothing in
/// the query was answered.
pub(crate) trait Forge: Send + Sync {
    /// Asks `host`, such as `github.com`, what `plan` asks, in one query.
    fn read<'a>(
        &'a self,
        host: &'a str,
        plan: &'a FetchPlan,
    ) -> Pin<Box<dyn Future<Output = Result<Batch, ReadError>> + Send + 'a>>;
}

/// Why a read got no answer. Each holds the reason in the words of `gh`, the
/// OS or GitHub.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ReadError {
    /// There is no `gh` to run on the daemon's machine.
    #[error("{0}")]
    NotInstalled(String),
    /// The host refused `gh`'s token, or `gh` has none for it: signing in
    /// with `gh auth login --hostname <host>` would help.
    #[error("{0}")]
    SignedOut(String),
    /// Anything else: the network, the deadline, GitHub refusing the query.
    #[error("{0}")]
    Failed(String),
}

/// How a `gh` run failed to give an answer that reads, for [`classify`].
#[derive(Debug)]
pub(crate) enum Ended<'a> {
    /// The OS couldn't start it.
    Unstarted(&'a io::Error),
    /// It ran and said this, on its error output or in GitHub's errors.
    Said(&'a str),
}

/// Why a run that ended as `ended` got no answer. A token GitHub refused
/// reaches `gh` as a raw 401, and a missing sign-in makes `gh` suggest
/// `gh auth login`: both read as signed out.
pub(crate) fn classify(ended: Ended<'_>) -> ReadError {
    match ended {
        Ended::Unstarted(error) if error.kind() == io::ErrorKind::NotFound => {
            ReadError::NotInstalled(error.to_string())
        }
        Ended::Unstarted(error) => ReadError::Failed(error.to_string()),
        Ended::Said(why) => {
            let line = why.lines().map(str::trim).find(|line| !line.is_empty());
            let line = line.unwrap_or_default();
            let line = line.strip_prefix("gh: ").unwrap_or(line).to_owned();
            let signed_out = why.contains("401")
                || why.contains("Bad credentials")
                || why.contains("gh auth login");
            if signed_out {
                ReadError::SignedOut(line)
            } else {
                ReadError::Failed(line)
            }
        }
    }
}

/// How long `gh` may take to answer before a read fails: a tripwire, not a
/// guess. rho-ui measured a batched PR query at 0.6 to 0.9 s, and the
/// dearest query the poller sends (25 full details) answered in 4.9 s on
/// 2026-11-22, a twelfth of this. It covers finding `gh` through the login
/// shell too.
pub(crate) const DEADLINE: Duration = Duration::from_secs(60);

/// What `gh` runs with: never asking, without update notices, spinners or
/// colors.
const QUIET: [(&str, &str); 4] = [
    ("GH_PROMPT_DISABLED", "1"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("GH_SPINNER_DISABLED", "1"),
    ("NO_COLOR", "1"),
];

/// The production [`Forge`]: `gh api graphql` on the daemon's machine, as
/// the account `gh` is signed in to for each host.
pub(crate) struct GhCli {
    /// The `PATH` to look for `gh` on.
    path: Option<OsString>,
    /// The user's login shell, asked for `gh` when `path` lacks it: a daemon
    /// started from a desktop launcher inherits its bare `PATH`.
    shell: Option<OsString>,
    /// Where `gh` was found; forgotten when it no longer starts, so a later
    /// read looks again.
    found: Mutex<Option<PathBuf>>,
}

impl Default for GhCli {
    /// Looks for `gh` on the daemon's own `PATH` and `SHELL`.
    fn default() -> Self {
        Self::new(std::env::var_os("PATH"), std::env::var_os("SHELL"))
    }
}

impl GhCli {
    /// A reader that looks for `gh` on `path`, else through the login shell
    /// `shell`.
    pub(crate) fn new(path: Option<OsString>, shell: Option<OsString>) -> Self {
        Self {
            path,
            shell,
            found: Mutex::new(None),
        }
    }

    /// Where `gh` is: on the `PATH`, else where the login shell finds it.
    pub(crate) async fn find(&self) -> Option<PathBuf> {
        if let Some(gh) = find_on("gh", self.path.as_deref()) {
            return Some(gh);
        }
        login_shell_gh(self.shell.as_deref()?).await
    }

    /// Where `gh` is, found once and then remembered.
    async fn program(&self) -> Option<PathBuf> {
        let remembered = self
            .found
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if remembered.is_some() {
            return remembered;
        }
        let found = self.find().await?;
        *self.found.lock().unwrap_or_else(PoisonError::into_inner) = Some(found.clone());
        Some(found)
    }

    async fn read_now(&self, host: &str, plan: &FetchPlan) -> Result<Batch, ReadError> {
        let Some(gh) = self.program().await else {
            return Err(ReadError::NotInstalled(
                "gh is not on the daemon's PATH, and its login shell doesn't find it either"
                    .to_owned(),
            ));
        };
        let query = gh_query::query(plan);
        let mut command = Command::new(&gh);
        command
            .args(["api", "graphql", "--hostname", host, "-f"])
            .arg(format!("query={query}"))
            .envs(QUIET)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        // `output` pipes stdout and stderr.
        let output = match command.output().await {
            Ok(output) => output,
            Err(error) => {
                if error.kind() == io::ErrorKind::NotFound {
                    *self.found.lock().unwrap_or_else(PoisonError::into_inner) = None;
                }
                return Err(classify(Ended::Unstarted(&error)));
            }
        };
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.stdout.trim_ascii().is_empty() && stderr.trim().is_empty() {
            return Err(ReadError::Failed(match output.status.code() {
                Some(code) => format!("gh exited with status {code} and printed nothing"),
                None => "gh was stopped by a signal and printed nothing".to_owned(),
            }));
        }
        // `gh` exits 1 when part of the query failed, as when a repository
        // isn't found, yet prints the rest of the answer: its status says
        // nothing the answer doesn't.
        gh_query::parse(plan, &output.stdout, &stderr)
    }
}

impl Forge for GhCli {
    fn read<'a>(
        &'a self,
        host: &'a str,
        plan: &'a FetchPlan,
    ) -> Pin<Box<dyn Future<Output = Result<Batch, ReadError>> + Send + 'a>> {
        Box::pin(async move {
            match tokio::time::timeout(DEADLINE, self.read_now(host, plan)).await {
                Ok(read) => read,
                Err(_) => {
                    let seconds = DEADLINE.as_secs();
                    Err(ReadError::Failed(format!(
                        "gh didn't answer within {seconds} s"
                    )))
                }
            }
        })
    }
}

/// Where the login shell `shell` finds `gh`: its last line, after whatever
/// its login files print, if that is an absolute path to a program (an
/// alias or function prints something else). Unix only, as `$SHELL` is.
async fn login_shell_gh(shell: &OsStr) -> Option<PathBuf> {
    if cfg!(windows) {
        return None;
    }
    let output = Command::new(shell)
        .args(["-l", "-c", "command -v gh"])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    let line = output
        .stdout
        .split(|&byte| byte == b'\n')
        .rfind(|line| !line.trim_ascii().is_empty())?;
    let gh = PathBuf::from(std::str::from_utf8(line).ok()?.trim());
    (gh.is_absolute() && is_executable_file(&gh)).then_some(gh)
}

#[cfg(test)]
#[path = "gh_tests.rs"]
mod tests;
