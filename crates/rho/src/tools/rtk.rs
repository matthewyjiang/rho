use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

use crate::tool::ToolResult;
use serde_json::json;
use tokio::{io::AsyncWriteExt, process::Command};

const REWRITE_TIMEOUT: Duration = Duration::from_secs(2);
static DISCOVER_LOG_NAME: LazyLock<String> =
    LazyLock::new(|| format!("rho-{}-{}.jsonl", std::process::id(), uuid::Uuid::new_v4()));

pub fn is_available() -> bool {
    let Ok(output) = std::process::Command::new("rtk").arg("--version").output() else {
        return false;
    };
    output.status.success() && supports_rewrite(&String::from_utf8_lossy(&output.stdout))
}

fn supports_rewrite(version: &str) -> bool {
    let Some((major, minor, _patch)) = parse_version(version) else {
        return true;
    };
    major > 0 || minor >= 23
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let version = version
        .trim()
        .strip_prefix("rtk ")
        .unwrap_or(version.trim());
    let mut parts = version.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.split_whitespace().next()?.parse().ok()?,
    ))
}

pub async fn rewrite(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty()
        || command.starts_with("rtk ")
        || std::env::var("RTK_DISABLED").ok().as_deref() == Some("1")
    {
        return None;
    }

    let output = tokio::time::timeout(
        REWRITE_TIMEOUT,
        Command::new("rtk").arg("rewrite").arg(command).output(),
    )
    .await
    .ok()?
    .ok()?;

    let code = output.status.code()?;
    if code != 0 && code != 3 {
        return None;
    }

    let rewritten = String::from_utf8(output.stdout).ok()?;
    let rewritten = rewritten.trim();
    (!rewritten.is_empty() && rewritten != command).then(|| rewritten.to_string())
}

pub(super) async fn log_execution(cwd: &Path, command: &str, result: &ToolResult) {
    let Some(projects_dir) = discover_projects_dir() else {
        return;
    };
    let _ = log_execution_in_projects_dir(&projects_dir, cwd, command, result).await;
}

fn discover_projects_dir() -> Option<PathBuf> {
    let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::paths::home_dir().map(|home| home.join(".claude")))?;
    Some(claude_dir.join("projects"))
}

async fn log_execution_in_projects_dir(
    projects_dir: &Path,
    cwd: &Path,
    command: &str,
    result: &ToolResult,
) -> std::io::Result<()> {
    let project_dir = projects_dir
        .join(encode_project_path(cwd))
        .join("rho-sessions");
    tokio::fs::create_dir_all(&project_dir).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&project_dir, std::fs::Permissions::from_mode(0o700)).await?;
    }

    let path = project_dir.join(DISCOVER_LOG_NAME.as_str());
    let tool_use_id = format!("rho-{}", uuid::Uuid::new_v4());
    let assistant = json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": "Bash",
                "input": { "command": command }
            }]
        }
    });
    let output_size = " ".repeat(result.content.len());
    let result_entry = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": output_size,
                "is_error": !result.ok
            }]
        }
    });
    let mut entry = serde_json::to_vec(&assistant).map_err(std::io::Error::other)?;
    entry.push(b'\n');
    entry.extend(serde_json::to_vec(&result_entry).map_err(std::io::Error::other)?);
    entry.push(b'\n');

    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(&entry).await?;
    file.flush().await
}

fn encode_project_path(path: &Path) -> String {
    const SANITIZED_CHARS: &[char] = &['/', '.', '_', '\\', ':', ' ', '[', ']'];

    path.to_string_lossy()
        .chars()
        .map(|ch| {
            if !ch.is_ascii() || SANITIZED_CHARS.contains(&ch) {
                '-'
            } else {
                ch
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "rtk_tests.rs"]
mod tests;
