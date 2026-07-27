use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Canonical transcript name inside a session folder.
pub(super) const SESSION_TRANSCRIPT_FILE_NAME: &str = "session.jsonl";
/// Sidecar directory name for web-access blobs inside a session folder.
pub(super) const SESSION_WEB_DIR_NAME: &str = "web";
/// Delegated run artifact directory inside a folder-layout session.
pub(super) const SESSION_SUBAGENTS_DIR_NAME: &str = "subagents";

/// One durable session unit on disk: folder layout or legacy flat transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionUnit {
    /// `<created-at>_<id>/session.jsonl` plus optional `web/` sidecar.
    Folder { dir: PathBuf },
    /// Legacy flat `<created-at>_<id>.jsonl` with optional `*.web/` companion.
    LegacyFile { path: PathBuf },
}

impl SessionUnit {
    /// Parses a workspace directory entry or an already-resolved transcript path.
    pub(super) fn from_path(path: &Path) -> Option<Self> {
        if path.is_dir() {
            let transcript = path.join(SESSION_TRANSCRIPT_FILE_NAME);
            return transcript.is_file().then(|| Self::Folder {
                dir: path.to_path_buf(),
            });
        }
        if is_folder_transcript(path) {
            return Some(Self::Folder {
                dir: path.parent()?.to_path_buf(),
            });
        }
        if is_legacy_transcript(path) {
            return Some(Self::LegacyFile {
                path: path.to_path_buf(),
            });
        }
        None
    }

    pub(super) fn transcript_path(&self) -> PathBuf {
        match self {
            Self::Folder { dir } => dir.join(SESSION_TRANSCRIPT_FILE_NAME),
            Self::LegacyFile { path } => path.clone(),
        }
    }

    pub(super) fn web_dir(&self) -> PathBuf {
        match self {
            Self::Folder { dir } => dir.join(SESSION_WEB_DIR_NAME),
            Self::LegacyFile { path } => {
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("session");
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(format!("{stem}.web"))
            }
        }
    }

    pub(super) fn subagents_dir(&self) -> Option<PathBuf> {
        match self {
            Self::Folder { dir } => Some(dir.join(SESSION_SUBAGENTS_DIR_NAME)),
            Self::LegacyFile { .. } => None,
        }
    }

    pub(super) fn unit_name(&self) -> Option<&str> {
        match self {
            Self::Folder { dir } => dir.file_name()?.to_str(),
            Self::LegacyFile { path } => path.file_stem()?.to_str(),
        }
    }

    pub(super) fn id(&self) -> Option<String> {
        self.unit_name()?
            .rsplit_once('_')
            .map(|(_, id)| id.to_string())
    }

    pub(super) fn created_at_from_name(&self) -> Option<u64> {
        self.unit_name()?
            .split_once('_')
            .and_then(|(timestamp, _)| parse_timestamp(timestamp))
    }

    /// Removes this session unit from disk (transcript, web sidecar, nested dirs).
    pub(super) fn delete_from_disk(&self) -> anyhow::Result<()> {
        match self {
            Self::Folder { dir } => {
                if dir.exists() {
                    fs::remove_dir_all(dir)?;
                }
            }
            Self::LegacyFile { path } => {
                let web_dir = self.web_dir();
                if path.exists() {
                    fs::remove_file(path)?;
                }
                if web_dir.exists() {
                    fs::remove_dir_all(&web_dir)?;
                }
            }
        }
        Ok(())
    }
}

/// Resolves a workspace entry or transcript path to the session JSONL file.
pub(super) fn resolve_transcript_path(path: &Path) -> Option<PathBuf> {
    SessionUnit::from_path(path).map(|unit| unit.transcript_path())
}

/// Web-access sidecar directory for a session transcript path.
pub(super) fn session_web_dir(transcript_path: &Path) -> Option<PathBuf> {
    SessionUnit::from_path(transcript_path).map(|unit| unit.web_dir())
}

pub(super) fn session_id_from_path(path: &Path) -> Option<String> {
    SessionUnit::from_path(path)?.id()
}

fn is_folder_transcript(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(SESSION_TRANSCRIPT_FILE_NAME)
}

fn is_legacy_transcript(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.contains('_'))
        && !is_folder_transcript(path)
}

pub(super) fn session_root() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("sessions"))
}

pub(super) fn session_dir_in_root(session_root: &Path, cwd: &Path) -> PathBuf {
    session_root.join(workspace_key(cwd))
}

pub(super) fn ensure_session_dir(session_root: &Path, cwd: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(session_root)?;
    set_private_dir_permissions(session_root)?;

    let dir = session_dir_in_root(session_root, cwd);
    fs::create_dir_all(&dir)?;
    set_private_dir_permissions(&dir)?;
    Ok(dir)
}

pub(super) fn set_private_dir_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub(super) fn set_private_file_permissions(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

pub(super) fn workspace_key(cwd: &Path) -> String {
    format!("{}-{:016x}", encode_cwd(cwd), stable_path_hash(cwd))
}

pub(super) fn encode_cwd(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn stable_path_hash(path: &Path) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    path.to_string_lossy()
        .as_bytes()
        .iter()
        .fold(FNV_OFFSET, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
        })
}

pub(super) fn parse_timestamp(timestamp: &str) -> Option<u64> {
    timestamp.parse().ok()
}

pub(super) fn timestamp() -> String {
    unix_timestamp_secs().to_string()
}

pub(super) fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
