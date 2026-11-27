//! Git repository sources for `rho plugins install` and `rho plugins update`.
//!
//! A source names a repository URL, an optional ref, and an optional package
//! subdirectory inside the repository. GitHub browser links
//! (`https://github.com/<owner>/<repo>/tree/<ref>/<path>`) parse into all three
//! parts; other git URLs fetch the remote default branch at the repository
//! root.
//!
//! Fetching shells out to the user's `git`, so existing credentials,
//! `insteadOf` rewrites, and proxies apply. A fetch only downloads and checks
//! out files; it never executes package code.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::Command,
};

use super::{contain, state::GitRecord};

/// URL prefixes `git` may fetch from. Other schemes are rejected.
const REMOTE_PREFIXES: &[&str] = &["https://", "http://", "ssh://", "git://", "file://", "git@"];

/// Hosts whose browser links parse as `<owner>/<repo>[/tree/<ref>[/<path>]]`.
const GITHUB_HOSTS: &[&str] = &["github.com", "www.github.com"];

/// Whether `git` may ask the user for credentials or to trust a host key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitPrompts {
    /// Through the terminal, as `rho plugins` in a shell lets it.
    Terminal,
    /// Never: git and the ssh it runs fail instead of asking, on a terminal or
    /// through an askpass program or credential manager window
    /// ([`forbid_prompts`]). The daemon fetches this way, so a login prompt
    /// can neither stop git on the terminal that started the daemon nor wait
    /// on a dialog nobody is watching.
    Never,
}

/// Whose ssh command git runs for an ssh remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SshCommand {
    /// git's default `ssh`, which [`forbid_prompts`] runs in batch mode.
    Default,
    /// One the user set with `GIT_SSH_COMMAND`, `core.sshCommand`, or
    /// `GIT_SSH`, left as it is: it may be a wrapper that takes no ssh
    /// options. Without a terminal, a display, or an askpass program, an ssh
    /// it runs still has nothing to prompt with.
    UserConfigured,
}

/// Variables naming a program that asks the user something, or a display
/// one would open a window on: an ssh older than 8.4, which ignores
/// `SSH_ASKPASS_REQUIRE`, runs its built-in askpass when a display is set.
const PROMPT_ENV: [&str; 4] = ["GIT_ASKPASS", "SSH_ASKPASS", "DISPLAY", "WAYLAND_DISPLAY"];

/// The ssh command git runs for [`SshCommand::Default`].
const BATCH_SSH_COMMAND: &str = "ssh -o BatchMode=yes";

/// Make `command`, a git command line before its subcommand, fail rather
/// than prompt: no terminal (its own session on Unix, and
/// `GIT_TERMINAL_PROMPT=0`), no askpass program (`core.askPass` empty, which
/// also keeps git from falling back to `SSH_ASKPASS`), no ssh askpass
/// (`SSH_ASKPASS_REQUIRE=never`, and no display for an older ssh), no Git
/// Credential Manager window, and ssh in batch mode unless the user chose
/// the ssh command. Credential helpers still answer from what they stored.
fn forbid_prompts(command: &mut Command, ssh: SshCommand) {
    for name in PROMPT_ENV {
        command.env_remove(name);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .args(["-c", "core.askPass="]);
    match ssh {
        SshCommand::Default => {
            command.env("GIT_SSH_COMMAND", BATCH_SSH_COMMAND);
        }
        SshCommand::UserConfigured => {}
    }
    #[cfg(unix)]
    rho_tools::process_session::start_new_session(command);
}

/// Where `rho plugins install` reads a package from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallSource {
    Local(PathBuf),
    Git(GitSource),
}

impl InstallSource {
    /// Classify an install argument as a local path or a git source.
    ///
    /// Arguments with a URL scheme (any case) or a `git@` prefix are always
    /// remotes; unsupported schemes are rejected rather than read as paths.
    /// A scheme-less argument is local when that path exists (relative to
    /// `cwd`), so a directory named like `github.com/...` still installs
    /// locally; otherwise it may be a scheme-less GitHub link.
    pub(crate) fn parse(arg: &OsStr, cwd: &Path) -> anyhow::Result<Self> {
        let path = Path::new(arg);
        let local = || Ok(Self::Local(path.to_path_buf()));
        let Some(arg) = arg.to_str() else {
            return local();
        };
        // Explicit remotes never consult the filesystem, so a planted
        // `https:/...` directory cannot stand in for a URL.
        if let Some(source) = GitSource::parse_remote(arg) {
            return source.map(Self::Git);
        }
        if cwd.join(path).exists() {
            return local();
        }
        if let Some(source) = github_source(arg) {
            return source.map(Self::Git);
        }
        local()
    }
}

/// A validated repository location plus the ref and package directory to
/// install.
///
/// Fields are private so every instance, including ones rebuilt from
/// `plugins.toml` (which can arrive with cloned repositories), went through
/// [`GitSource::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitSource {
    url: String,
    git_ref: Option<String>,
    subdir: Option<String>,
}

impl GitSource {
    /// Validate and build a source.
    ///
    /// `git_ref` is a branch, tag, or commit; `None` follows the remote default
    /// branch. `subdir` is the package directory relative to the repository
    /// root; `None` is the root. Rejects values that could reach `git` as
    /// options, escape the checkout, or persist credentials (the URL is
    /// recorded in `plugins.toml` and printed).
    pub(crate) fn new(
        url: String,
        git_ref: Option<String>,
        subdir: Option<String>,
    ) -> anyhow::Result<Self> {
        if !REMOTE_PREFIXES.iter().any(|prefix| url.starts_with(prefix)) {
            anyhow::bail!(
                "unsupported git URL `{url}`: expected one of {}",
                REMOTE_PREFIXES.join(", ")
            );
        }
        if embeds_credentials(&url) {
            anyhow::bail!("{CREDENTIALS_REFUSED}");
        }
        if let Some(git_ref) = &git_ref {
            if git_ref.is_empty() || git_ref.starts_with('-') {
                anyhow::bail!("invalid git ref `{git_ref}`");
            }
        }
        if let Some(subdir) = &subdir {
            let path = Path::new(subdir);
            if subdir.is_empty()
                || !path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                anyhow::bail!(
                    "invalid plugin path `{subdir}`: must be relative to the repository root without `.` or `..`"
                );
            }
        }
        Ok(Self {
            url,
            git_ref,
            subdir,
        })
    }

    /// Parse `url` as `rho plugins install` reads a git source, without
    /// ever reading it as a local path: an explicit remote, or a GitHub
    /// link with its scheme left off.
    pub(crate) fn parse(url: &str) -> anyhow::Result<Self> {
        Self::parse_remote(url)
            .or_else(|| github_source(url))
            .unwrap_or_else(|| {
                anyhow::bail!(
                    "`{url}` is not a git URL: expected one of {}, or a github.com link",
                    REMOTE_PREFIXES.join(", ")
                )
            })
    }

    /// `arg` as a source when it is spelled as a remote (a URL scheme in
    /// any case, or `git@`); `None` otherwise.
    fn parse_remote(arg: &str) -> Option<anyhow::Result<Self>> {
        let remote = remote_spelling(arg)?;
        Some(github_source(&remote).unwrap_or_else(|| Self::new(remote, None, None)))
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn subdir(&self) -> Option<&str> {
        self.subdir.as_deref()
    }

    /// Whether the URL points at GitHub, where `/tree/<ref>/<path>` links work.
    pub(crate) fn is_github(&self) -> bool {
        url::Url::parse(&self.url).is_ok_and(|url| is_github_web(&url))
    }

    /// State record for this source at `commit`.
    pub(crate) fn record(&self, commit: String) -> GitRecord {
        GitRecord {
            url: self.url.clone(),
            git_ref: self.git_ref.clone(),
            subdir: self.subdir.clone(),
            commit,
        }
    }

    /// Human-readable location, e.g. `https://github.com/o/r (ref main, path plugins/x)`.
    pub(crate) fn describe(&self) -> String {
        let mut details = Vec::new();
        if let Some(git_ref) = &self.git_ref {
            details.push(format!("ref {git_ref}"));
        }
        if let Some(subdir) = &self.subdir {
            details.push(format!("path {subdir}"));
        }
        if details.is_empty() {
            self.url.clone()
        } else {
            format!("{} ({})", self.url, details.join(", "))
        }
    }
}

impl TryFrom<&GitRecord> for GitSource {
    type Error = anyhow::Error;

    /// Rebuild a recorded source, re-running every validation rule.
    fn try_from(record: &GitRecord) -> anyhow::Result<Self> {
        Self::new(
            record.url.clone(),
            record.git_ref.clone(),
            record.subdir.clone(),
        )
    }
}

/// `arg` spelled for `git` when it names a remote: a `scheme://` URL with the
/// scheme lowercased (git matches transports case-sensitively), or an
/// scp-style `git@host:path`.
fn remote_spelling(arg: &str) -> Option<String> {
    if let Some((scheme, rest)) = arg.split_once("://") {
        let is_scheme = scheme
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if is_scheme {
            return Some(format!("{}://{rest}", scheme.to_ascii_lowercase()));
        }
    }
    arg.starts_with("git@").then(|| arg.to_string())
}

const CREDENTIALS_REFUSED: &str =
    "refusing a git URL with embedded credentials; use a git credential helper or SSH key instead";

/// Whether `url` carries a password, or any userinfo over HTTP(S) where a
/// bare username is commonly an access token. `ssh://git@host` stays allowed.
fn embeds_credentials(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        // scp-style `git@host:path` is not a URL and has no password slot.
        return false;
    };
    parsed.password().is_some()
        || matches!(parsed.scheme(), "http" | "https") && !parsed.username().is_empty()
}

/// An HTTP(S) URL on a GitHub web host (host match is case-insensitive).
fn is_github_web(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url
            .host_str()
            .is_some_and(|host| GITHUB_HOSTS.contains(&host))
}

/// Parse `arg` as a GitHub web link (`https://` optional); `None` when it is
/// not one.
///
/// `url::Url` decides the scheme and host. The path is split from the raw
/// text instead, because URL parsing resolves `..` segments before the
/// package path check could reject them. Ref and path segments are
/// percent-decoded, so links GitHub encodes (`my%20plugin`, `feature%2Fx`)
/// resolve to the real names.
///
/// GitHub shows `/` in ref names unencoded, so in a plain tree link the first
/// segment after `tree/` is taken as the ref.
fn github_source(arg: &str) -> Option<anyhow::Result<GitSource>> {
    // scp-style remotes are SSH; prefixing them with `https://` would read
    // `git@github.com:1234/repo` as a username and port.
    if arg.starts_with("git@") {
        return None;
    }
    let with_scheme = if arg.contains("://") {
        arg.to_string()
    } else {
        format!("https://{arg}")
    };
    if !is_github_web(&url::Url::parse(&with_scheme).ok()?) {
        return None;
    }
    Some(github_source_from(&with_scheme))
}

fn github_source_from(url: &str) -> anyhow::Result<GitSource> {
    if embeds_credentials(url) {
        anyhow::bail!("{CREDENTIALS_REFUSED}");
    }
    let after_scheme = url.split_once("://").map_or("", |(_, rest)| rest);
    let path = after_scheme
        .find('/')
        .map_or("", |start| &after_scheme[start..]);
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let (Some(owner), Some(repo)) = (segments.next(), segments.next()) else {
        anyhow::bail!("unsupported GitHub URL: expected github.com/<owner>/<repo>");
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let (git_ref, subdir) = match segments.next() {
        None => (None, None),
        Some("tree") => {
            let Some(git_ref) = segments.next() else {
                anyhow::bail!("unsupported GitHub URL: `tree/` needs a ref");
            };
            let subdir = segments.map(decode_segment).collect::<anyhow::Result<Vec<_>>>()?;
            let subdir = subdir.join("/");
            (
                Some(decode_segment(git_ref)?),
                (!subdir.is_empty()).then_some(subdir),
            )
        }
        Some(other) => anyhow::bail!(
            "unsupported GitHub URL segment `{other}`: use github.com/<owner>/<repo>/tree/<ref>/<path>"
        ),
    };
    GitSource::new(
        format!("https://github.com/{owner}/{repo}"),
        git_ref,
        subdir,
    )
}

/// Percent-decode one URL path segment.
fn decode_segment(segment: &str) -> anyhow::Result<String> {
    percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| anyhow::anyhow!("GitHub URL segment `{segment}` is not valid UTF-8"))
}

/// A shallow checkout of one commit in a temporary directory.
///
/// The `.git` directory is removed after checkout so package copies never
/// carry repository metadata. Everything is deleted on drop.
pub(crate) struct Checkout {
    /// Owns `repo` and the empty hooks directory beside it.
    _dir: tempfile::TempDir,
    repo: PathBuf,
    pub(crate) commit: String,
}

impl Checkout {
    /// Package directory inside the checkout, contained after resolving symlinks.
    pub(crate) fn package_dir(&self, source: &GitSource) -> anyhow::Result<PathBuf> {
        let Some(subdir) = source.subdir() else {
            return Ok(self.repo.clone());
        };
        let canonical =
            contain::canonical_root(&self.repo).map_err(|error| anyhow::anyhow!(error))?;
        contain::contained_path(&canonical, &canonical.join(subdir)).map_err(|_| {
            anyhow::anyhow!(
                "path `{subdir}` does not exist in {} at {}",
                source.url(),
                short_commit(&self.commit)
            )
        })
    }
}

/// Shallow-fetch `source` and check out its commit.
///
/// Git runs with an empty template and hooks directory, so user-level
/// `post-checkout` hooks (which can act on repository config such as
/// `.pre-commit-config.yaml`) never run against fetched content.
pub(crate) fn fetch(source: &GitSource, prompts: GitPrompts) -> anyhow::Result<Checkout> {
    let dir = tempfile::Builder::new().prefix("rho-plugin-").tempdir()?;
    let no_hooks = dir.path().join("no-hooks");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&no_hooks)?;
    std::fs::create_dir(&repo)?;
    let mut git = Git {
        repo: &repo,
        no_hooks: &no_hooks,
        prompts: None,
    };
    let template = format!("--template={}", no_hooks.display());
    git.run(&["init", "--quiet", &template])?;
    git.prompts = match prompts {
        GitPrompts::Terminal => None,
        GitPrompts::Never => Some(git.ssh_command()?),
    };
    let target = source.git_ref.as_deref().unwrap_or("HEAD");
    git.run(&[
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "--",
        source.url(),
        target,
    ])
    .map_err(|error| anyhow::anyhow!("could not fetch {}: {error}", source.describe()))?;
    git.run(&[
        "-c",
        "advice.detachedHead=false",
        "checkout",
        "--quiet",
        "FETCH_HEAD",
    ])?;
    let commit = git.run(&["rev-parse", "HEAD"])?;
    std::fs::remove_dir_all(repo.join(".git"))?;
    Ok(Checkout {
        _dir: dir,
        repo,
        commit,
    })
}

/// Abbreviated commit for display.
pub(crate) fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// `git` confined to one temporary repository with hooks disabled.
struct Git<'a> {
    repo: &'a Path,
    no_hooks: &'a Path,
    /// Set to forbid prompts ([`GitPrompts::Never`]), with whose ssh
    /// command runs.
    prompts: Option<SshCommand>,
}

impl Git<'_> {
    /// `git -C <repo> <args>`, as [`Git::run`] runs it.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        for name in crate::git::LOCATION_ENV {
            command.env_remove(name);
        }
        if let Some(ssh) = self.prompts {
            forbid_prompts(&mut command, ssh);
        }
        command
            .arg("-C")
            .arg(self.repo)
            .arg("-c")
            .arg(format!("core.hooksPath={}", self.no_hooks.display()))
            .args(args);
        command
    }

    /// Whose ssh command git would run: the user's when their environment
    /// or git configuration sets one.
    fn ssh_command(&self) -> anyhow::Result<SshCommand> {
        if ["GIT_SSH_COMMAND", "GIT_SSH"]
            .iter()
            .any(|name| std::env::var_os(name).is_some())
        {
            return Ok(SshCommand::UserConfigured);
        }
        // `git config --get` exits 1 when the key is unset.
        let output = self
            .command(&["config", "--get", "core.sshCommand"])
            .output()
            .map_err(spawn_error)?;
        match output.status.code() {
            Some(0) => Ok(SshCommand::UserConfigured),
            Some(1) => Ok(SshCommand::Default),
            _ => anyhow::bail!(
                "git config failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    /// Run `git -C <repo> <args>` and return trimmed stdout.
    fn run(&self, args: &[&str]) -> anyhow::Result<String> {
        let output = self.command(args).output().map_err(spawn_error)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "git {} failed: {}",
                args.first().copied().unwrap_or_default(),
                stderr.trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn spawn_error(error: std::io::Error) -> anyhow::Error {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            anyhow::anyhow!("git is not installed; install git to use repository plugin sources")
        }
        _ => anyhow::anyhow!("could not run git: {error}"),
    }
}

#[cfg(test)]
#[path = "git_source_tests.rs"]
mod tests;
