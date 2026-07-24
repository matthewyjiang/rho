use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use rho_tools::tool::ToolError;

static CONTENT_STORE: OnceLock<Mutex<HashMap<String, StoredContent>>> = OnceLock::new();

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct StoredContent {
    pub(super) kind: String,
    pub(super) items: Vec<StoredItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct StoredItem {
    pub(super) url: Option<String>,
    pub(super) query: Option<String>,
    pub(super) title: Option<String>,
    pub(super) content: String,
    pub(super) metadata: Value,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ContentAvailability {
    pub(super) snippets: bool,
    pub(super) sources: bool,
}

/// Session-scoped (or fallback) root for durable web-access blobs.
///
/// Owned by the app tool set / interactive runtime and injected into web tools.
/// Not a process-global "active session" side channel.
#[derive(Clone, Debug, Default)]
pub struct WebAccessStore {
    state: Arc<Mutex<WebAccessStoreState>>,
}

#[derive(Debug, Default)]
struct WebAccessStoreState {
    session_root: Option<PathBuf>,
    #[cfg(test)]
    override_root: Option<PathBuf>,
}

impl WebAccessStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Points durable web blobs at the active session sidecar directory.
    pub fn bind_session(&self, root: Option<PathBuf>) {
        self.state
            .lock()
            .expect("web access store lock poisoned")
            .session_root = root;
    }

    /// Durable sidecar root for web-access blobs and GitHub clones.
    ///
    /// Preference order:
    /// 1. test override
    /// 2. bound session `web/` directory
    /// 3. process data-dir fallback
    /// 4. temp dir when no Rho home is available
    pub fn root(&self) -> PathBuf {
        let state = self.state.lock().expect("web access store lock poisoned");
        #[cfg(test)]
        if let Some(path) = state.override_root.clone() {
            return path;
        }
        if let Some(path) = state.session_root.clone() {
            return path;
        }
        default_web_access_cache_root()
    }

    pub(super) fn store(
        &self,
        response_id: String,
        content: StoredContent,
    ) -> Result<(), ToolError> {
        write_at(&self.root(), &response_id, &content)?;
        content_store()
            .lock()
            .expect("content store lock poisoned")
            .insert(response_id, content);
        Ok(())
    }

    pub(super) fn load(&self, response_id: &str) -> Result<StoredContent, ToolError> {
        validate_response_id(response_id)?;
        if let Some(content) = content_store()
            .lock()
            .expect("content store lock poisoned")
            .get(response_id)
            .cloned()
        {
            return Ok(content);
        }
        read_at(&self.root(), response_id)
    }

    pub(super) fn create_private_dir_all(&self, path: &Path) -> Result<(), ToolError> {
        fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let root = self.root();
            if root.exists() {
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            }
            if path.exists() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn with_root(path: PathBuf) -> Self {
        let store = Self::new();
        store.state.lock().expect("web access store lock poisoned").override_root = Some(path);
        store
    }
}

pub(super) fn content_availability(items: &[StoredItem]) -> ContentAvailability {
    ContentAvailability {
        snippets: items.iter().any(|item| {
            matches!(
                item.metadata.get("contentKind").and_then(Value::as_str),
                Some("snippet") | Some("snippet_with_fetch_warning")
            )
        }),
        sources: items.iter().any(|item| {
            item.metadata.get("contentKind").and_then(Value::as_str) == Some("source_page")
        }),
    }
}

pub(super) fn new_response_id() -> String {
    Uuid::new_v4().simple().to_string()
}

pub(super) fn validate_response_id(response_id: &str) -> Result<(), ToolError> {
    let valid = response_id.len() == 32
        && response_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if valid {
        Ok(())
    } else {
        Err(ToolError::Message(
            "invalid responseId: expected 32 lowercase hexadecimal characters".into(),
        ))
    }
}

/// Lists exact selector keys an agent may pass to `get_search_content`.
pub(super) fn available_selectors(stored: &StoredContent) -> String {
    if stored.items.is_empty() {
        return "no stored items".into();
    }
    let mut lines = Vec::with_capacity(stored.items.len());
    let mut query_index = 0usize;
    for (index, item) in stored.items.iter().enumerate() {
        let mut parts = vec![format!("urlIndex={index}")];
        if let Some(url) = item.url.as_deref() {
            parts.push(format!("url={url}"));
        }
        if let Some(query) = item.query.as_deref() {
            parts.push(format!("query={query:?}"));
            parts.push(format!("queryIndex={query_index}"));
            query_index += 1;
        }
        lines.push(format!("- {}", parts.join(" ")));
    }
    lines.join("\n")
}

fn content_store() -> &'static Mutex<HashMap<String, StoredContent>> {
    CONTENT_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn default_web_access_cache_root() -> PathBuf {
    // Used only when no session is bound (tests, pre-session tool calls, automation).
    crate::paths::rho_dir()
        .map(|dir| dir.join("web-access"))
        .unwrap_or_else(|_| std::env::temp_dir().join("rho-web-access"))
}

fn write_at(root: &Path, response_id: &str, content: &StoredContent) -> Result<(), ToolError> {
    let path = stored_content_path(root, response_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if root.exists() {
                fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
            }
            if parent.exists() {
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    let serialized = serde_json::to_string(content)
        .map_err(|err| ToolError::Message(format!("failed to serialize stored content: {err}")))?;
    write_private_file(&path, serialized.as_bytes())
}

fn read_at(root: &Path, response_id: &str) -> Result<StoredContent, ToolError> {
    let path = stored_content_path(root, response_id)?;
    match fs::read_to_string(&path) {
        Ok(content) => parse_stored_content(&content),
        Err(_) => read_legacy_temp(response_id),
    }
}

fn read_legacy_temp(response_id: &str) -> Result<StoredContent, ToolError> {
    let legacy = std::env::temp_dir()
        .join("rho-web-access")
        .join("content")
        .join(format!("{response_id}.json"));
    let content = fs::read_to_string(&legacy).map_err(|_| {
        ToolError::Message(format!(
            "unknown responseId: {response_id}. Stored web content is a sidecar blob under the Rho data directory and is available only while that cache file exists. Re-run fetch_content or web_search for the original URL or query."
        ))
    })?;
    parse_stored_content(&content)
}

fn parse_stored_content(content: &str) -> Result<StoredContent, ToolError> {
    serde_json::from_str(content)
        .map_err(|err| ToolError::Message(format!("stored content was not valid JSON: {err}")))
}

fn stored_content_path(root: &Path, response_id: &str) -> Result<PathBuf, ToolError> {
    validate_response_id(response_id)?;
    Ok(root.join("content").join(format!("{response_id}.json")))
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, contents)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
