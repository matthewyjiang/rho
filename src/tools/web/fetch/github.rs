use std::{
    fs,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::{json, Value};
use tokio::process::Command;
use url::Url;

use crate::tool::{truncate, ToolError};

use super::{FetchedTarget, PREVIEW_BYTES};
use crate::tools::web::{
    process,
    storage::{create_private_dir_all, web_access_cache_root},
    util::{safe_path_component, to_pretty_json},
};

const LARGE_REPO_THRESHOLD_KB: u64 = 350 * 1024;

pub(super) async fn fetch(
    client: &reqwest::Client,
    github: &GitHubTarget,
    force_clone: bool,
) -> Result<FetchedTarget, ToolError> {
    if github.kind == GitHubKind::Commit {
        return github_api_fallback(client, github, None).await;
    }

    let repo_api = format!(
        "https://api.github.com/repos/{}/{}",
        github.owner, github.repo
    );
    let repo_size_kb = github_api_json(client, &repo_api)
        .await
        .ok()
        .and_then(|value| value.get("size").and_then(Value::as_u64));
    let oversized = repo_size_kb.is_some_and(|size| size > LARGE_REPO_THRESHOLD_KB);
    if oversized && !force_clone {
        return github_api_fallback(client, github, repo_size_kb).await;
    }

    match ensure_github_clone(github).await {
        Ok(local_path) => read_github_clone(github, &local_path).await,
        Err(_) => github_api_fallback(client, github, repo_size_kb).await,
    }
}

async fn github_api_fallback(
    client: &reqwest::Client,
    github: &GitHubTarget,
    repo_size_kb: Option<u64>,
) -> Result<FetchedTarget, ToolError> {
    let api_url = github_api_content_url(github);
    let content = match github.kind {
        GitHubKind::Blob => github_api_file_content(client, &api_url).await?,
        GitHubKind::Root | GitHubKind::Tree | GitHubKind::Commit => {
            to_pretty_json(&github_api_json(client, &api_url).await?)
        }
    };
    Ok(FetchedTarget {
        title: Some(format!("{}/{}", github.owner, github.repo)),
        preview: json!({
            "type": "github_api_fallback",
            "repo": format!("{}/{}", github.owner, github.repo),
            "reason": repo_size_kb.map(|size| format!("repo size {size}KB exceeds 350MB threshold")).unwrap_or_else(|| "clone unavailable".into()),
            "canForceClone": true,
            "preview": truncate(content.clone(), PREVIEW_BYTES)
        }),
        content,
        metadata: json!({"mode": "api_fallback", "repoSizeKb": repo_size_kb}),
    })
}

fn github_api_content_url(github: &GitHubTarget) -> String {
    match github.kind {
        GitHubKind::Root | GitHubKind::Tree | GitHubKind::Blob => format!(
            "https://api.github.com/repos/{}/{}/contents/{}{}",
            github.owner,
            github.repo,
            github.path,
            github
                .ref_name
                .as_ref()
                .map(|ref_name| format!("?ref={ref_name}"))
                .unwrap_or_default()
        ),
        GitHubKind::Commit => format!(
            "https://api.github.com/repos/{}/{}/commits/{}",
            github.owner,
            github.repo,
            github.ref_name.as_deref().unwrap_or("HEAD")
        ),
    }
}

async fn github_api_file_content(client: &reqwest::Client, url: &str) -> Result<String, ToolError> {
    let value = github_api_json(client, url).await?;
    let encoding = value.get("encoding").and_then(Value::as_str);
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ToolError::Message("GitHub API response did not include file content".into())
        })?;
    if encoding == Some("base64") {
        let compact = content.lines().collect::<String>();
        let bytes = BASE64_STANDARD.decode(compact).map_err(|err| {
            ToolError::Message(format!("GitHub file content was not base64: {err}"))
        })?;
        return String::from_utf8(bytes).map_err(ToolError::Utf8);
    }
    Ok(content.to_string())
}

async fn github_api_json(client: &reqwest::Client, url: &str) -> Result<Value, ToolError> {
    let mut request = client.get(url).header("User-Agent", "rho-coding-agent");
    if let Ok(token) = github_token() {
        request = request.bearer_auth(token);
    }
    request
        .send()
        .await
        .map_err(|err| ToolError::Message(format!("GitHub API request failed: {err}")))?
        .error_for_status()
        .map_err(|err| ToolError::Message(format!("GitHub API request failed: {err}")))?
        .json()
        .await
        .map_err(|err| ToolError::Message(format!("GitHub API response was not JSON: {err}")))
}

fn github_token() -> Result<String, std::env::VarError> {
    std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN"))
}

async fn ensure_github_clone(github: &GitHubTarget) -> Result<PathBuf, ToolError> {
    let cache_root = web_access_cache_root()
        .join(std::process::id().to_string())
        .join("github")
        .join(safe_path_component(&github.owner))
        .join(safe_path_component(&github.repo));
    let ref_key = github.ref_name.as_deref().unwrap_or("HEAD");
    let local_path = cache_root.join(safe_path_component(ref_key));
    create_private_dir_all(&cache_root)?;
    if local_path.join(".git").is_dir() {
        checkout_github_ref(github, &local_path).await?;
        return Ok(local_path);
    }

    let repo_slug = format!("{}/{}", github.owner, github.repo);
    let clone_url = format!("https://github.com/{repo_slug}.git");
    let mut command = if let Ok(token) = github_token() {
        let mut command = Command::new("gh");
        command
            .arg("repo")
            .arg("clone")
            .arg(&repo_slug)
            .arg(&local_path)
            .arg("--")
            .arg("--depth")
            .arg("1");
        if std::env::var_os("GH_TOKEN").is_none() {
            command.env("GH_TOKEN", token);
        }
        command
    } else {
        let mut command = Command::new("git");
        command
            .arg("clone")
            .arg("--depth")
            .arg("1")
            .arg(clone_url)
            .arg(&local_path);
        command
    };
    process::run(
        &mut command,
        &format!("git clone for {}/{}", github.owner, github.repo),
    )
    .await?;
    checkout_github_ref(github, &local_path).await?;
    Ok(local_path)
}

async fn checkout_github_ref(github: &GitHubTarget, local_path: &Path) -> Result<(), ToolError> {
    let Some(ref_name) = github.ref_name.as_deref() else {
        return Ok(());
    };
    let mut fetch = Command::new("git");
    fetch
        .arg("-C")
        .arg(local_path)
        .arg("fetch")
        .arg("--depth")
        .arg("1")
        .arg("origin")
        .arg(ref_name);
    process::run(
        &mut fetch,
        &format!(
            "git fetch for {}/{} ref {ref_name}",
            github.owner, github.repo
        ),
    )
    .await?;
    let mut checkout = Command::new("git");
    checkout
        .arg("-C")
        .arg(local_path)
        .arg("checkout")
        .arg("--detach")
        .arg("FETCH_HEAD");
    process::run(
        &mut checkout,
        &format!(
            "git checkout for {}/{} ref {ref_name}",
            github.owner, github.repo
        ),
    )
    .await?;
    Ok(())
}

async fn read_github_clone(
    github: &GitHubTarget,
    local_path: &Path,
) -> Result<FetchedTarget, ToolError> {
    let target_path = local_path.join(&github.path);
    let commit = process::output(
        Command::new("git")
            .arg("-C")
            .arg(local_path)
            .arg("rev-parse")
            .arg("HEAD"),
        "git rev-parse HEAD",
    )
    .await
    .ok()
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());

    match github.kind {
        GitHubKind::Root | GitHubKind::Tree => {
            let dir = if github.kind == GitHubKind::Root {
                local_path
            } else {
                &target_path
            };
            let tree = directory_listing(dir)?;
            let readme = find_readme(dir).and_then(|path| fs::read_to_string(path).ok());
            let content = format!(
                "localPath: {}\ncommit: {}\n\n{}{}",
                local_path.display(),
                commit.clone().unwrap_or_else(|| "unknown".into()),
                tree,
                readme
                    .as_ref()
                    .map(|readme| format!("\n\nREADME:\n{readme}"))
                    .unwrap_or_default()
            );
            Ok(FetchedTarget {
                title: Some(format!("{}/{}", github.owner, github.repo)),
                preview: json!({
                    "type": "github_repo",
                    "localPath": local_path,
                    "commit": commit,
                    "tree": tree,
                    "readmePreview": readme.map(|readme| truncate(readme, PREVIEW_BYTES))
                }),
                content,
                metadata: json!({"mode": "clone", "localPath": local_path, "commit": commit}),
            })
        }
        GitHubKind::Blob => {
            let content = fs::read_to_string(&target_path)?;
            Ok(FetchedTarget {
                title: Some(github.path.clone()),
                preview: json!({
                    "type": "github_file",
                    "localPath": target_path,
                    "commit": commit,
                    "preview": truncate(content.clone(), PREVIEW_BYTES)
                }),
                content,
                metadata: json!({"mode": "clone", "localPath": local_path, "commit": commit}),
            })
        }
        GitHubKind::Commit => github_api_fallback_sync_notice(github, local_path, commit),
    }
}

fn github_api_fallback_sync_notice(
    github: &GitHubTarget,
    local_path: &Path,
    commit: Option<String>,
) -> Result<FetchedTarget, ToolError> {
    let content = format!(
        "Commit URLs are handled via GitHub API in fetch_content. Clone is available at {} with HEAD {}.",
        local_path.display(),
        commit.as_deref().unwrap_or("unknown")
    );
    Ok(FetchedTarget {
        title: Some(format!("{}/{} commit", github.owner, github.repo)),
        preview: json!({"type": "github_commit", "warning": content}),
        content,
        metadata: json!({"mode": "commit_notice", "localPath": local_path, "commit": commit}),
    })
}

fn directory_listing(path: &Path) -> Result<String, ToolError> {
    let mut entries = fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let suffix = if file_type.is_dir() { "/" } else { "" };
            Ok(format!("{}{}", entry.file_name().to_string_lossy(), suffix))
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    entries.sort();
    Ok(entries.join("\n"))
}

fn find_readme(path: &Path) -> Option<PathBuf> {
    ["README.md", "README.txt", "README"]
        .into_iter()
        .map(|name| path.join(name))
        .find(|path| path.is_file())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tools::web) enum GitHubKind {
    Root,
    Tree,
    Blob,
    Commit,
}

#[derive(Debug)]
pub(in crate::tools::web) struct GitHubTarget {
    pub(in crate::tools::web) owner: String,
    pub(in crate::tools::web) repo: String,
    pub(in crate::tools::web) kind: GitHubKind,
    pub(in crate::tools::web) ref_name: Option<String>,
    pub(in crate::tools::web) path: String,
}

pub(in crate::tools::web) fn parse_url(input: &str) -> Option<GitHubTarget> {
    let url = Url::parse(input).ok()?;
    if url.host_str()? != "github.com" {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.len() < 2 {
        return None;
    }
    let owner = segments[0].to_string();
    let repo = segments[1].trim_end_matches(".git").to_string();
    match segments.get(2).copied() {
        None | Some("") => Some(GitHubTarget {
            owner,
            repo,
            kind: GitHubKind::Root,
            ref_name: None,
            path: String::new(),
        }),
        Some("tree") | Some("blob") => {
            let kind = if segments[2] == "tree" {
                GitHubKind::Tree
            } else {
                GitHubKind::Blob
            };
            let (ref_name, path) = split_github_ref_and_path(kind, &segments[3..]);
            Some(GitHubTarget {
                owner,
                repo,
                kind,
                ref_name,
                path,
            })
        }
        Some("commit") => Some(GitHubTarget {
            owner,
            repo,
            kind: GitHubKind::Commit,
            ref_name: segments.get(3).map(|value| (*value).to_string()),
            path: String::new(),
        }),
        _ => None,
    }
}

fn split_github_ref_and_path(_kind: GitHubKind, segments: &[&str]) -> (Option<String>, String) {
    if segments.is_empty() {
        return (None, String::new());
    }
    if segments.len() == 1 {
        return (Some(segments[0].to_string()), String::new());
    }

    let split_at = find_github_path_start(segments).unwrap_or(1);
    (
        Some(segments[..split_at].join("/")),
        segments[split_at..].join("/"),
    )
}

fn find_github_path_start(segments: &[&str]) -> Option<usize> {
    (1..segments.len()).find(|index| is_common_github_path_start(segments[*index]))
}

fn is_common_github_path_start(segment: &str) -> bool {
    matches!(
        segment,
        "src"
            | "docs"
            | "doc"
            | "test"
            | "tests"
            | "crates"
            | "packages"
            | "package"
            | "examples"
            | "example"
            | "scripts"
            | "script"
            | "tools"
            | "tool"
            | "app"
            | "apps"
            | "lib"
            | "libs"
            | "cmd"
            | "components"
            | "component"
            | "internal"
            | "pkg"
            | ".github"
    )
}
