use std::{
    fs,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::{json, Value};
use url::Url;

use rho_tools::tool::{truncate, ToolError};

use super::{FetchedTarget, PREVIEW_BYTES};
use crate::tools::web::util::to_pretty_json;

pub(in crate::tools::web) async fn fetch_via_api(
    client: &reqwest::Client,
    github: &GitHubTarget,
) -> Result<FetchedTarget, ToolError> {
    let api_url = api_url(github);
    let content = match github.kind {
        GitHubKind::Blob => github_api_file_content(client, &api_url).await?,
        GitHubKind::Root | GitHubKind::Tree | GitHubKind::Commit => {
            to_pretty_json(&github_api_json(client, &api_url).await?)
        }
    };
    Ok(FetchedTarget {
        title: Some(format!("{}/{}", github.owner, github.repo)),
        preview: json!({
            "type": "github_api",
            "repo": format!("{}/{}", github.owner, github.repo),
            "canForceClone": github.kind != GitHubKind::Commit,
            "preview": truncate(content.clone(), PREVIEW_BYTES)
        }),
        content,
        metadata: json!({"mode": "github_api"}),
    })
}

pub(in crate::tools::web) fn api_url(github: &GitHubTarget) -> String {
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

pub(in crate::tools::web) fn clone_url(github: &GitHubTarget) -> String {
    format!("https://github.com/{}/{}.git", github.owner, github.repo)
}

pub(in crate::tools::web) fn authenticated_clone_url(github: &GitHubTarget) -> String {
    match github_token() {
        Ok(token) => format!(
            "https://x-access-token:{token}@github.com/{}/{}.git",
            github.owner, github.repo
        ),
        Err(_) => clone_url(github),
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

pub(in crate::tools::web) async fn read_clone(
    github: &GitHubTarget,
    local_path: &Path,
) -> Result<FetchedTarget, ToolError> {
    let target_path = local_path.join(&github.path);
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
                "GitHub repository {}/{}\n\n{}{}",
                github.owner,
                github.repo,
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
                    "tree": tree,
                    "readmePreview": readme.map(|readme| truncate(readme, PREVIEW_BYTES))
                }),
                content,
                metadata: json!({"mode": "clone"}),
            })
        }
        GitHubKind::Blob => {
            let content = fs::read_to_string(&target_path)?;
            Ok(FetchedTarget {
                title: Some(github.path.clone()),
                preview: json!({
                    "type": "github_file",
                    "path": github.path,
                    "preview": truncate(content.clone(), PREVIEW_BYTES)
                }),
                content,
                metadata: json!({"mode": "clone"}),
            })
        }
        GitHubKind::Commit => Err(ToolError::Message(
            "commit URLs must use the GitHub API fetch plan".into(),
        )),
    }
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
