use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use rho_tools::tool::ToolError;

static CONTENT_STORE: OnceLock<Mutex<HashMap<String, StoredContent>>> = OnceLock::new();
static CACHE_ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

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

pub(super) fn store(response_id: String, content: StoredContent) -> Result<(), ToolError> {
    write(&response_id, &content)?;
    content_store()
        .lock()
        .expect("content store lock poisoned")
        .insert(response_id, content);
    Ok(())
}

pub(super) fn load(response_id: &str) -> Result<StoredContent, ToolError> {
    validate_response_id(response_id)?;
    if let Some(content) = content_store()
        .lock()
        .expect("content store lock poisoned")
        .get(response_id)
        .cloned()
    {
        return Ok(content);
    }
    read(response_id)
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

/// Durable sidecar root for web-access blobs and GitHub clones.
///
/// Lives under the Rho data directory when available so content survives process
/// restarts without being stuffed into session transcripts. Falls back to the
/// process temp directory only when the data root cannot be resolved.
pub(super) fn web_access_cache_root() -> PathBuf {
    if let Some(path) = CACHE_ROOT_OVERRIDE
        .lock()
        .expect("web access cache root lock poisoned")
        .clone()
    {
        return path;
    }
    default_web_access_cache_root()
}

pub(super) fn create_private_dir_all(path: &Path) -> Result<(), ToolError> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let root = web_access_cache_root();
        if root.exists() {
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        }
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Lists exact selector keys an agent may pass to `get_search_content`.
pub(super) fn available_selectors(stored: &StoredContent) -> String {
    let mut lines = Vec::new();
    if stored.items.is_empty() {
        return "no stored items".into();
    }
    for (index, item) in stored.items.iter().enumerate() {
        let mut parts = vec![format!("urlIndex={index}")];
        if let Some(url) = item.url.as_deref() {
            parts.push(format!("url={url}"));
        }
        if let Some(query) = item.query.as_deref() {
            parts.push(format!("query={query:?}"));
        }
        if item.query.is_some() {
            let query_index = stored
                .items
                .iter()
                .take(index + 1)
                .filter(|candidate| candidate.query.is_some())
                .count()
                .saturating_sub(1);
            parts.push(format!("queryIndex={query_index}"));
        }
        lines.push(format!("- {}", parts.join(" ")));
    }
    lines.join("\n")
}

fn content_store() -> &'static Mutex<HashMap<String, StoredContent>> {
    CONTENT_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn default_web_access_cache_root() -> PathBuf {
    crate::paths::rho_dir()
        .map(|dir| dir.join("web-access"))
        .unwrap_or_else(|_| std::env::temp_dir().join("rho-web-access"))
}

fn write(response_id: &str, content: &StoredContent) -> Result<(), ToolError> {
    let path = stored_content_path(response_id)?;
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let serialized = serde_json::to_string(content)
        .map_err(|err| ToolError::Message(format!("failed to serialize stored content: {err}")))?;
    write_private_file(&path, serialized.as_bytes())
}

fn read(response_id: &str) -> Result<StoredContent, ToolError> {
    let path = stored_content_path(response_id)?;
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

fn stored_content_path(response_id: &str) -> Result<PathBuf, ToolError> {
    validate_response_id(response_id)?;
    Ok(web_access_cache_root()
        .join("content")
        .join(format!("{response_id}.json")))
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
pub(super) struct CacheRootGuard {
    previous: Option<PathBuf>,
}

#[cfg(test)]
impl CacheRootGuard {
    pub(super) fn set(path: PathBuf) -> Self {
        let previous = CACHE_ROOT_OVERRIDE
            .lock()
            .expect("web access cache root lock poisoned")
            .replace(path);
        Self { previous }
    }
}

#[cfg(test)]
impl Drop for CacheRootGuard {
    fn drop(&mut self) {
        *CACHE_ROOT_OVERRIDE
            .lock()
            .expect("web access cache root lock poisoned") = self.previous.take();
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
