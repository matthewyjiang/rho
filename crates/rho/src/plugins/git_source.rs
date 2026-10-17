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
    path::{Component, Path, PathBuf},
    process::Command,
};

use super::contain;

/// URL prefixes treated as git remotes. Anything else is a local path.
const REMOTE_PREFIXES: &[&str] = &["https://", "http://", "ssh://", "git://", "file://", "git@"];

/// GitHub web prefixes whose path is parsed as `<owner>/<repo>[/tree/<ref>[/<path>]]`.
const GITHUB_PREFIXES: &[&str] = &[
    "https://github.com/",
    "http://github.com/",
    "https://www.github.com/",
    "github.com/",
];

/// Where `rho plugins install` reads a package from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallSource {
    Local(PathBuf),
    Git(GitSource),
}

impl InstallSource {
    /// Classify an install argument as a local path or a git source.
    pub(crate) fn parse(arg: &str) -> anyhow::Result<Self> {
        if let Some(rest) = GITHUB_PREFIXES
            .iter()
            .find_map(|prefix| arg.strip_prefix(prefix))
        {
            return parse_github_path(rest).map(Self::Git);
        }
        if REMOTE_PREFIXES.iter().any(|prefix| arg.starts_with(prefix)) {
            let source = GitSource {
                url: arg.to_string(),
                git_ref: None,
                subdir: None,
            };
            source.validate()?;
            return Ok(Self::Git(source));
        }
        Ok(Self::Local(PathBuf::from(arg)))
    }
}

/// A repository location plus the ref and package directory to install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitSource {
    pub(crate) url: String,
    /// Branch, tag, or commit to fetch. `None` follows the remote default branch.
    pub(crate) git_ref: Option<String>,
    /// Package directory relative to the repository root. `None` is the root.
    pub(crate) subdir: Option<String>,
}

impl GitSource {
    /// Reject values that could reach `git` as options or escape the checkout.
    ///
    /// Runs on parsed arguments and again on sources loaded from
    /// `plugins.toml`, because project state files arrive with cloned
    /// repositories.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if !REMOTE_PREFIXES
            .iter()
            .any(|prefix| self.url.starts_with(prefix))
        {
            anyhow::bail!(
                "unsupported git URL `{}`: expected one of {}",
                self.url,
                REMOTE_PREFIXES.join(", ")
            );
        }
        if let Some(git_ref) = &self.git_ref {
            if git_ref.is_empty() || git_ref.starts_with('-') {
                anyhow::bail!("invalid git ref `{git_ref}`");
            }
        }
        if let Some(subdir) = &self.subdir {
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
        Ok(())
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

/// Parse the part of a GitHub web URL after the host.
///
/// The first segment after `tree/` is the ref, so refs that contain `/` are
/// not expressible through a tree URL.
fn parse_github_path(rest: &str) -> anyhow::Result<GitSource> {
    let path = rest.split(['?', '#']).next().unwrap_or_default();
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
            let subdir = segments.collect::<Vec<_>>().join("/");
            (
                Some(git_ref.to_string()),
                (!subdir.is_empty()).then_some(subdir),
            )
        }
        Some(other) => anyhow::bail!(
            "unsupported GitHub URL segment `{other}`: use github.com/<owner>/<repo>/tree/<ref>/<path>"
        ),
    };
    let source = GitSource {
        url: format!("https://github.com/{owner}/{repo}"),
        git_ref,
        subdir,
    };
    source.validate()?;
    Ok(source)
}

/// A shallow checkout of one commit in a temporary directory.
///
/// The `.git` directory is removed after checkout so package copies never
/// carry repository metadata. The directory is deleted on drop.
pub(crate) struct Checkout {
    dir: tempfile::TempDir,
    pub(crate) commit: String,
}

impl Checkout {
    /// Package directory inside the checkout, contained after resolving symlinks.
    pub(crate) fn package_dir(&self, source: &GitSource) -> anyhow::Result<PathBuf> {
        let root = self.dir.path();
        let Some(subdir) = &source.subdir else {
            return Ok(root.to_path_buf());
        };
        let canonical = contain::canonical_root(root).map_err(|error| anyhow::anyhow!(error))?;
        contain::contained_path(&canonical, &canonical.join(subdir)).map_err(|_| {
            anyhow::anyhow!(
                "path `{subdir}` does not exist in {} at {}",
                source.url,
                short_commit(&self.commit)
            )
        })
    }
}

/// Shallow-fetch `source` and check out its commit.
pub(crate) fn fetch(source: &GitSource) -> anyhow::Result<Checkout> {
    source.validate()?;
    let dir = tempfile::Builder::new().prefix("rho-plugin-").tempdir()?;
    let target = source.git_ref.as_deref().unwrap_or("HEAD");
    run_git(dir.path(), &["init", "--quiet"])?;
    run_git(
        dir.path(),
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            "--",
            &source.url,
            target,
        ],
    )
    .map_err(|error| anyhow::anyhow!("could not fetch {}: {error}", source.describe()))?;
    run_git(
        dir.path(),
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--quiet",
            "FETCH_HEAD",
        ],
    )?;
    let commit = run_git(dir.path(), &["rev-parse", "HEAD"])?;
    std::fs::remove_dir_all(dir.path().join(".git"))?;
    Ok(Checkout { dir, commit })
}

/// Abbreviated commit for display.
pub(crate) fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// Run `git -C <dir> <args>` and return trimmed stdout.
fn run_git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                anyhow::anyhow!(
                    "git is not installed; install git to use repository plugin sources"
                )
            }
            _ => anyhow::anyhow!("could not run git: {error}"),
        })?;
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

#[cfg(test)]
#[path = "git_source_tests.rs"]
mod tests;
