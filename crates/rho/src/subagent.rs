//! Durable status and attachment artifacts for delegated agent runs.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const RESULT_FILE_NAME: &str = "result.json";
pub const LOG_FILE_NAME: &str = "log.txt";
pub const ATTACHMENT_FILE_NAME: &str = "events.jsonl";

/// State machine for a subagent run, persisted in the result file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    #[default]
    Starting,
    Running,
    Ok,
    Error,
    Stopped,
}

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Ok | Self::Error | Self::Stopped)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Stopped => "stopped",
        }
    }
}

/// Contents of the `--output-file` a subagent writes atomically as it runs.
///
/// The parent process reads this file for status checks and completion
/// detection; the pane or log output is display-only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunStatus {
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_error: Option<String>,
}

/// Writes the status file atomically (temp file + rename) so readers never
/// observe a torn write.
pub fn write_status(path: &Path, status: &RunStatus) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_vec_pretty(status)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(RESULT_FILE_NAME);
    let temp = path.with_file_name(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = create_private_file(&temp)?;
        file.write_all(&contents)?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn read_status(path: &Path) -> Option<RunStatus> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn directory(id: &str) -> anyhow::Result<PathBuf> {
    validate_id(id)?;
    Ok(crate::paths::rho_dir()?.join("subagents").join(id))
}

fn validate_id(id: &str) -> anyhow::Result<()> {
    if id.len() != 6 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid subagent id '{id}': expected 6 hexadecimal characters");
    }
    Ok(())
}

pub(crate) fn create_private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn secure_directory(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a trusted directory", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
