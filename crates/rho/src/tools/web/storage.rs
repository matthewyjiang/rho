use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use rho_tools::tool::ToolError;

/// In-memory copies of sidecar blobs. Disk remains the source of truth.
///
/// 32 recent responses or 16 MiB of text and serialized metadata, whichever
/// binds first. This payload budget excludes allocator/container overhead.
/// A single oversized blob stays on disk without flushing the other entries.
const MEMORY_ENTRY_LIMIT: usize = 32;
const MEMORY_BYTE_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(super) struct StoredContent {
    pub(super) kind: String,
    pub(super) items: Vec<StoredItem>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub(super) struct StoredItem {
    pub(super) url: Option<String>,
    pub(super) query: Option<String>,
    pub(super) title: Option<String>,
    pub(super) content: String,
    pub(super) metadata: Value,
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
    /// Identity, not a wrapping counter: in-flight I/O keeps its old binding alive.
    binding: Arc<()>,
    memory: MemoryCache,
    #[cfg(test)]
    override_root: Option<PathBuf>,
}

impl WebAccessStoreState {
    fn root(&self) -> PathBuf {
        #[cfg(test)]
        if let Some(path) = self.override_root.clone() {
            return path;
        }
        self.session_root
            .clone()
            .unwrap_or_else(default_web_access_cache_root)
    }
}

/// Session-scoped LRU of stored web bodies. Eviction never deletes sidecar files.
#[derive(Debug)]
struct MemoryCache {
    entries: HashMap<String, CachedContent>,
    /// Oldest entry at the front.
    order: VecDeque<String>,
    bytes: usize,
    entry_limit: usize,
    byte_limit: usize,
}

#[derive(Clone, Debug)]
struct CachedContent {
    content: Arc<StoredContent>,
    bytes: usize,
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new(MEMORY_ENTRY_LIMIT, MEMORY_BYTE_LIMIT)
    }
}

impl MemoryCache {
    fn new(entry_limit: usize, byte_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            entry_limit: entry_limit.max(1),
            byte_limit: byte_limit.max(1),
        }
    }

    fn get(&mut self, response_id: &str) -> Option<Arc<StoredContent>> {
        let content = Arc::clone(&self.entries.get(response_id)?.content);
        self.touch(response_id);
        Some(content)
    }

    fn insert(&mut self, response_id: String, content: Arc<StoredContent>) {
        let bytes = memory_bytes(&content);
        self.remove(&response_id);
        if bytes > self.byte_limit {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.entry_limit
                || self.bytes.saturating_add(bytes) > self.byte_limit)
        {
            self.evict_oldest();
        }
        self.bytes = self.bytes.saturating_add(bytes);
        self.order.push_back(response_id.clone());
        self.entries
            .insert(response_id, CachedContent { content, bytes });
    }

    #[cfg(test)]
    fn contains(&self, response_id: &str) -> bool {
        self.entries.contains_key(response_id)
    }

    fn touch(&mut self, response_id: &str) {
        if let Some(index) = self
            .order
            .iter()
            .position(|existing| existing == response_id)
        {
            if let Some(id) = self.order.remove(index) {
                self.order.push_back(id);
            }
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(id) = self.order.pop_front() {
            self.remove_entry(&id);
        }
    }

    fn remove(&mut self, response_id: &str) {
        self.order.retain(|existing| existing != response_id);
        self.remove_entry(response_id);
    }

    fn remove_entry(&mut self, response_id: &str) {
        if let Some(entry) = self.entries.remove(response_id) {
            self.bytes = self.bytes.saturating_sub(entry.bytes);
        }
    }
}

fn memory_bytes(content: &StoredContent) -> usize {
    content.kind.len()
        + content
            .items
            .iter()
            .map(|item| {
                item.content.len()
                    + item.url.as_deref().map_or(0, str::len)
                    + item.query.as_deref().map_or(0, str::len)
                    + item.title.as_deref().map_or(0, str::len)
                    + metadata_bytes(&item.metadata)
            })
            .sum::<usize>()
}

/// Count JSON without allocating a second copy of large metadata values.
fn metadata_bytes(value: &Value) -> usize {
    #[derive(Default)]
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter::default();
    serde_json::to_writer(&mut counter, value).expect("Value serialization into a counting sink");
    counter.0
}

impl WebAccessStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Points durable web blobs at the active session sidecar directory.
    pub fn bind_session(&self, root: Option<PathBuf>) {
        let mut state = self.state.lock().expect("web access store lock poisoned");
        state.session_root = root;
        state.binding = Arc::new(());
        state.memory = MemoryCache::default();
    }

    /// Durable sidecar root for web-access blobs and GitHub clones.
    ///
    /// Preference order:
    /// 1. test override
    /// 2. bound session `web/` directory
    /// 3. process data-dir fallback
    /// 4. temp dir when no Rho home is available
    pub fn root(&self) -> PathBuf {
        self.state
            .lock()
            .expect("web access store lock poisoned")
            .root()
    }

    pub(super) fn store(
        &self,
        response_id: String,
        content: StoredContent,
    ) -> Result<(), ToolError> {
        self.store_with_writer(response_id, content, write_at)
    }

    fn store_with_writer(
        &self,
        response_id: String,
        content: StoredContent,
        write: impl FnOnce(&Path, &str, &StoredContent) -> Result<(), ToolError>,
    ) -> Result<(), ToolError> {
        let (root, binding) = {
            let state = self.state.lock().expect("web access store lock poisoned");
            (state.root(), Arc::clone(&state.binding))
        };
        write(&root, &response_id, &content)?;
        self.cache_if_current(&binding, response_id, Arc::new(content));
        Ok(())
    }

    /// Shares immutable bodies so selecting one item never copies its siblings.
    pub(super) fn load(&self, response_id: &str) -> Result<Arc<StoredContent>, ToolError> {
        self.load_with_reader(response_id, read_at)
    }

    fn load_with_reader(
        &self,
        response_id: &str,
        read: impl FnOnce(&Path, &str) -> Result<StoredContent, ToolError>,
    ) -> Result<Arc<StoredContent>, ToolError> {
        validate_response_id(response_id)?;
        let (root, binding) = {
            let mut state = self.state.lock().expect("web access store lock poisoned");
            if let Some(content) = state.memory.get(response_id) {
                return Ok(content);
            }
            (state.root(), Arc::clone(&state.binding))
        };
        let content = Arc::new(read(&root, response_id)?);
        self.cache_if_current(&binding, response_id.to_owned(), Arc::clone(&content));
        Ok(content)
    }

    fn cache_if_current(
        &self,
        binding: &Arc<()>,
        response_id: String,
        content: Arc<StoredContent>,
    ) {
        let mut state = self.state.lock().expect("web access store lock poisoned");
        if Arc::ptr_eq(binding, &state.binding) {
            state.memory.insert(response_id, content);
        }
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
        store
            .state
            .lock()
            .expect("web access store lock poisoned")
            .override_root = Some(path);
        store
    }

    #[cfg(test)]
    fn memory_contains(&self, response_id: &str) -> bool {
        self.state
            .lock()
            .expect("web access store lock poisoned")
            .memory
            .contains(response_id)
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
        // Only a missing blob means the id may still live in the legacy location.
        // Other failures describe a real problem with the current blob and are
        // reported instead of being reduced to "unknown responseId".
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => read_legacy_temp(response_id),
        Err(error) => Err(ToolError::Message(format!(
            "failed to read stored web content {}: {error}",
            path.display()
        ))),
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
