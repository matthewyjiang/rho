use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{BufRead, BufReader, Seek},
    path::{Path, PathBuf},
};

use rho_sdk::{CompactionState, Revision, SessionSnapshot};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use uuid::Uuid;

#[path = "tree_legacy.rs"]
mod legacy;
#[path = "tree_restore.rs"]
mod restore;

use super::display_rows::{DisplayRowFormat, StoredDisplayRow};
use super::persistence::{
    next_revision, parse_timestamp, resume_normalized_history, session_id_from_path,
    validate_session_version, PersistedSessionState, SessionEntry, StoredDisplayMessage,
    EXPLICIT_TREE_VERSION, SESSION_VERSION,
};
use super::snapshot_delta::StoredSnapshotDelta;
use super::{SessionIndexRecord, SessionSummary};

/// Stable identity for one durable state in a session tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub(crate) struct NodeId(String);

impl NodeId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub(crate) fn from_string(id: impl Into<String>) -> anyhow::Result<Self> {
        let id = id.into();
        if id.is_empty() || id.trim() != id {
            anyhow::bail!("session node id cannot be empty or contain outer whitespace");
        }
        if id.len() > 128 {
            anyhow::bail!("session node id cannot exceed 128 bytes");
        }
        Ok(Self(id))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn legacy(offset: u64) -> Self {
        Self(format!("legacy:{offset:016x}"))
    }
}

impl<'de> Deserialize<'de> for NodeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::from_string(id).map_err(D::Error::custom)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionNodeKind {
    Commit,
    Compaction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StoredStateTransition {
    Snapshot { snapshot: Box<SessionSnapshot> },
    SnapshotDelta { delta: Box<StoredSnapshotDelta> },
}

impl StoredStateTransition {
    /// Model messages this record stores itself, which `StoredDisplayRow::Model`
    /// indexes: a delta's appended history or a snapshot's full history.
    pub(crate) fn model_messages(&self) -> &[rho_providers::model::Message] {
        match self {
            Self::Snapshot { snapshot } => snapshot.history(),
            Self::SnapshotDelta { delta } => delta.appended_history(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredCompactionFacts {
    pub(crate) previous_messages: usize,
    pub(crate) current_messages: usize,
    pub(crate) previous_tokens: u64,
    pub(crate) current_tokens: u64,
    pub(crate) cost_usd_micros: Option<u64>,
}

/// One tree node record. `D` is the display row type: stored rows as read
/// from or written to disk, or rows resolved to full messages once loaded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SessionNode<D = StoredDisplayRow> {
    pub(crate) id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_id: Option<NodeId>,
    pub(crate) timestamp: String,
    pub(crate) kind: SessionNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compaction_facts: Option<StoredCompactionFacts>,
    pub(crate) transition: StoredStateTransition,
    /// Display rows: stored rows may reference `transition` messages (see
    /// `display_rows`); loaded rows are full messages.
    /// `Vec::new` instead of `default` keeps serde from bounding `D: Default`.
    #[serde(default = "Vec::new")]
    pub(crate) display_messages: Vec<D>,
}

/// A node whose display rows are full messages.
pub(crate) type LoadedNode = SessionNode<StoredDisplayMessage>;

impl SessionNode {
    /// Resolves stored rows against this node's own transition; `format`
    /// comes from the file header.
    pub(crate) fn resolve_display(self, format: DisplayRowFormat) -> anyhow::Result<LoadedNode> {
        let display_messages = StoredDisplayRow::resolve(
            self.display_messages,
            self.transition.model_messages(),
            format,
            &self.timestamp,
            self.id.as_str(),
        )?;
        Ok(SessionNode {
            id: self.id,
            parent_id: self.parent_id,
            timestamp: self.timestamp,
            kind: self.kind,
            compaction_facts: self.compaction_facts,
            transition: self.transition,
            display_messages,
        })
    }
}

/// Compact facts kept on every node so picker labels and tree checks do not
/// need a full `PersistedSessionState` prefix.
#[derive(Clone, Debug)]
pub(crate) struct NodeFacts {
    pub(crate) revision: Revision,
    pub(crate) model_len: usize,
    pub(crate) has_snapshot: bool,
    pub(crate) compaction: CompactionState,
}

impl NodeFacts {
    fn from_state(state: &PersistedSessionState) -> Self {
        Self {
            revision: state.revision,
            model_len: state.model.len(),
            has_snapshot: state.snapshot.is_some(),
            compaction: state.compaction.clone(),
        }
    }
}

/// A node in the loaded tree: its record with display rows resolved, plus
/// the state facts derived while inserting it.
#[derive(Clone, Debug)]
pub(crate) struct RestoredNode {
    record: LoadedNode,
    facts: NodeFacts,
}

impl RestoredNode {
    pub(crate) fn id(&self) -> &NodeId {
        &self.record.id
    }

    pub(crate) fn parent_id(&self) -> Option<&NodeId> {
        self.record.parent_id.as_ref()
    }

    pub(crate) fn kind(&self) -> SessionNodeKind {
        self.record.kind
    }

    #[cfg(test)]
    pub(crate) fn timestamp(&self) -> &str {
        &self.record.timestamp
    }

    pub(crate) fn display_messages(&self) -> &[StoredDisplayMessage] {
        &self.record.display_messages
    }

    pub(crate) fn revision(&self) -> Revision {
        self.facts.revision
    }

    pub(super) fn facts(&self) -> &NodeFacts {
        &self.facts
    }

    pub(super) fn transition(&self) -> &StoredStateTransition {
        &self.record.transition
    }

    pub(super) fn compaction_facts(&self) -> Option<&StoredCompactionFacts> {
        self.record.compaction_facts.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionTreeFacts {
    pub(crate) node_count: usize,
    pub(crate) branch_count: usize,
    pub(crate) active_leaf_id: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionTreeItemKind {
    /// An empty durable root before any conversation turn.
    SessionStart,
    Turn,
    Compaction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionTreeItem {
    pub(crate) id: NodeId,
    pub(crate) depth: usize,
    pub(crate) kind: SessionTreeItemKind,
    pub(crate) first_user_text: Option<String>,
    pub(crate) compaction_facts: Option<StoredCompactionFacts>,
    pub(crate) active: bool,
    pub(crate) on_active_path: bool,
    pub(crate) ancestor_has_next_sibling: Vec<bool>,
    pub(crate) is_last_sibling: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionTree {
    nodes: HashMap<NodeId, RestoredNode>,
    children: HashMap<NodeId, Vec<NodeId>>,
    order: Vec<NodeId>,
    active_leaf_id: Option<NodeId>,
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    created_at: Option<u64>,
    updated_at: Option<u64>,
    version: Option<u32>,
    upgraded: bool,
    /// Cached count of parent nodes with more than one child, maintained in
    /// `insert_restored_node` so `facts()` avoids scanning every child list.
    branch_count: usize,
    /// Full state for `active_leaf_id` only. Other nodes reconstruct on demand.
    /// `None` while `active_replay` holds the leaf's pending state.
    active_state: Option<PersistedSessionState>,
    /// Sequential delta replay, during load and across saves. Load, `set_leaf`,
    /// or a full-snapshot node materializes it; readers copy it via `state_for`.
    /// At most one of `active_state` and `active_replay` is set.
    active_replay: Option<restore::ReplayState>,
}

impl SessionTree {
    pub(crate) fn load(path: &Path) -> anyhow::Result<Self> {
        Self::load_with_entry_visitor(path, |_| Ok(()))
    }

    fn observe_timestamp(&mut self, timestamp: u64) {
        self.updated_at = Some(self.updated_at.map_or(timestamp, |u| u.max(timestamp)));
    }

    pub(super) fn load_with_entry_visitor(
        path: &Path,
        mut visit: impl FnMut(&SessionEntry) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let file = fs::File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut tree = Self::default();
        let expected_session_id = session_id_from_path(path);
        let mut legacy_state = PersistedSessionState::default();
        let mut line = String::new();

        loop {
            line.clear();
            let offset = reader.stream_position()?;
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            let terminated = line.ends_with('\n');
            let entry = match serde_json::from_str::<SessionEntry>(&line) {
                Ok(entry) => entry,
                Err(error) if !terminated && error.is_eof() => break,
                Err(error) => return Err(error.into()),
            };
            visit(&entry)?;
            tree.apply_entry(
                entry,
                offset,
                &mut legacy_state,
                expected_session_id.as_deref(),
            )?;
        }
        tree.ensure_active_state()?;
        tree.validate_active_leaf()?;
        Ok(tree)
    }

    pub(crate) fn active_leaf_id(&self) -> Option<&NodeId> {
        self.active_leaf_id.as_ref()
    }

    /// Materialized state of the active leaf, as after [`Self::load`]. A saved
    /// delta node leaves it pending; cached-tree readers use
    /// [`Self::state_for`] instead.
    pub(crate) fn active_state(&self) -> Option<&PersistedSessionState> {
        debug_assert!(
            self.active_replay.is_none(),
            "active_state() read on a pending replay; use state_for, or ensure_active_state on an owned tree"
        );
        self.active_state.as_ref()
    }

    pub(crate) fn node(&self, id: &NodeId) -> Option<&RestoredNode> {
        self.nodes.get(id)
    }

    /// Metadata `key` from the most recently written node, whichever leaf is
    /// active. Selecting an older node writes no node, so session-scoped values
    /// saved with every snapshot survive tree navigation.
    pub(crate) fn newest_node_metadata(&self, key: &str) -> Option<&str> {
        let node = self.order.last().and_then(|id| self.nodes.get(id))?;
        match node.transition() {
            StoredStateTransition::Snapshot { snapshot } => snapshot.metadata().get(key),
            StoredStateTransition::SnapshotDelta { delta } => delta.metadata().get(key),
        }
        .map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn children(&self, id: &NodeId) -> &[NodeId] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }

    /// Test-only access to the full child map, used to benchmark the old
    /// scan-every-child-list approach against the cached `branch_count` field.
    #[cfg(test)]
    pub(crate) fn children_map(&self) -> &HashMap<NodeId, Vec<NodeId>> {
        &self.children
    }

    #[cfg(test)]
    pub(crate) fn nodes_in_storage_order(&self) -> impl Iterator<Item = &RestoredNode> {
        self.order.iter().filter_map(|id| self.nodes.get(id))
    }

    pub(crate) fn active_path(&self) -> anyhow::Result<Vec<&RestoredNode>> {
        let Some(id) = self.active_leaf_id.as_ref() else {
            return Ok(Vec::new());
        };
        self.path_to(id)
    }

    pub(crate) fn path_to(&self, target_id: &NodeId) -> anyhow::Result<Vec<&RestoredNode>> {
        let mut id = target_id;
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(id.clone()) {
                anyhow::bail!("session tree contains a cycle at node '{id}'");
            }
            let node = self
                .nodes
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{id}'"))?;
            path.push(node);
            let Some(parent_id) = node.parent_id() else {
                break;
            };
            id = parent_id;
        }
        path.reverse();
        Ok(path)
    }

    pub(crate) fn projected_display(
        &self,
        target_id: &NodeId,
    ) -> anyhow::Result<Vec<StoredDisplayMessage>> {
        let mut display = Vec::new();
        for node in self.path_to(target_id)? {
            display.extend(node.display_messages().iter().cloned());
            if node.kind() == SessionNodeKind::Compaction {
                let detail = if let Some(facts) = node.compaction_facts() {
                    let cost = facts.cost_usd_micros.map_or_else(String::new, |micros| {
                        format!("\n${:.6} compaction cost", micros as f64 / 1_000_000.0)
                    });
                    format!(
                        "\n{} → {} messages\n{} → {} estimated tokens{cost}",
                        facts.previous_messages,
                        facts.current_messages,
                        facts.previous_tokens,
                        facts.current_tokens,
                    )
                } else {
                    match (
                        node.facts().compaction.last_previous_tokens(),
                        node.facts().compaction.last_current_tokens(),
                    ) {
                        (Some(previous), Some(current)) => {
                            format!("\n{previous} → {current} estimated tokens")
                        }
                        _ => String::new(),
                    }
                };
                display.push(StoredDisplayMessage {
                    timestamp: node.record.timestamp.clone(),
                    message: rho_providers::model::Message::assistant_text(format!(
                        "◆ Compacted context{detail}"
                    )),
                });
            }
        }
        Ok(display)
    }

    pub(crate) fn facts(&self) -> SessionTreeFacts {
        SessionTreeFacts {
            node_count: self.nodes.len(),
            branch_count: self.branch_count,
            active_leaf_id: self.active_leaf_id.clone(),
        }
    }

    pub(crate) fn items(&self) -> anyhow::Result<Vec<SessionTreeItem>> {
        let active_path: HashSet<_> = self
            .active_path()?
            .into_iter()
            .map(|node| node.id().clone())
            .collect();
        let mut traversal = Vec::new();
        let roots = self
            .order
            .iter()
            .filter(|id| self.node(id).is_some_and(|node| node.parent_id().is_none()))
            .cloned()
            .collect::<Vec<_>>();
        let mut stack = roots
            .iter()
            .enumerate()
            .rev()
            .map(|(index, id)| (id.clone(), 0usize, Vec::new(), index + 1 == roots.len()))
            .collect::<Vec<_>>();
        while let Some((id, depth, ancestor_has_next_sibling, is_last_sibling)) = stack.pop() {
            let node = self
                .node(&id)
                .ok_or_else(|| anyhow::anyhow!("session tree is missing node '{id}'"))?;
            let kind = match node.kind() {
                // Classify from durable state, not missing user text: assistant-only
                // commits are still turns, while an empty root is the session start.
                SessionNodeKind::Commit
                    if node.parent_id().is_none()
                        && node.facts().model_len == 0
                        && node.display_messages().is_empty() =>
                {
                    SessionTreeItemKind::SessionStart
                }
                SessionNodeKind::Commit => SessionTreeItemKind::Turn,
                SessionNodeKind::Compaction => SessionTreeItemKind::Compaction,
            };
            let first_user_text = (kind == SessionTreeItemKind::Turn).then(|| {
                node.display_messages()
                    .iter()
                    .find_map(|stored| super::persistence::user_message_text(&stored.message))
                    .unwrap_or_else(|| "turn".into())
            });
            let compaction_facts = (kind == SessionTreeItemKind::Compaction).then(|| {
                node.compaction_facts()
                    .cloned()
                    .unwrap_or_else(|| StoredCompactionFacts {
                        previous_messages: node
                            .parent_id()
                            .and_then(|id| self.node(id))
                            .map_or(0, |parent| parent.facts().model_len),
                        current_messages: node.facts().model_len,
                        previous_tokens: node
                            .facts()
                            .compaction
                            .last_previous_tokens()
                            .unwrap_or_default(),
                        current_tokens: node
                            .facts()
                            .compaction
                            .last_current_tokens()
                            .unwrap_or_default(),
                        cost_usd_micros: None,
                    })
            });
            traversal.push(SessionTreeItem {
                id: node.id().clone(),
                depth,
                kind,
                first_user_text,
                compaction_facts,
                active: self.active_leaf_id() == Some(node.id()),
                on_active_path: active_path.contains(node.id()),
                ancestor_has_next_sibling: ancestor_has_next_sibling.clone(),
                is_last_sibling,
            });
            let children = self.children.get(&id).map_or(&[][..], Vec::as_slice);
            for (index, child_id) in children.iter().enumerate().rev() {
                let mut child_ancestors = ancestor_has_next_sibling.clone();
                if depth > 0 {
                    child_ancestors.push(!is_last_sibling);
                }
                stack.push((
                    child_id.clone(),
                    depth + 1,
                    child_ancestors,
                    index + 1 == children.len(),
                ));
            }
        }
        Ok(traversal)
    }

    pub(crate) fn effective_format_version(&self) -> u32 {
        if self.upgraded {
            EXPLICIT_TREE_VERSION
        } else {
            self.version.unwrap_or(SESSION_VERSION)
        }
    }

    pub(crate) fn needs_upgrade_marker(&self) -> bool {
        self.version
            .is_some_and(|version| version < EXPLICIT_TREE_VERSION)
            && !self.upgraded
    }

    /// Row encoding for nodes in this file. The header version fixes it, so
    /// resuming an older file keeps it readable by older binaries.
    pub(crate) fn display_row_format(&self) -> DisplayRowFormat {
        DisplayRowFormat::for_version(self.effective_format_version())
    }

    fn is_explicit_tree_version(&self) -> bool {
        self.version
            .is_some_and(|version| version >= EXPLICIT_TREE_VERSION)
    }

    pub(crate) fn apply_explicit_node(&mut self, node: SessionNode) -> anyhow::Result<()> {
        self.require_explicit_phase("node")?;
        if let Some(ts) = parse_timestamp(&node.timestamp) {
            self.observe_timestamp(ts);
        }
        // A delta node leaves its state as a pending replay: materializing
        // here would deep-copy the whole history on every save.
        self.insert_explicit_node(node)
    }

    pub(crate) fn apply_set_leaf(
        &mut self,
        target_id: NodeId,
        timestamp_str: &str,
    ) -> anyhow::Result<()> {
        self.require_explicit_phase("set_leaf")?;
        if let Some(ts) = parse_timestamp(timestamp_str) {
            self.observe_timestamp(ts);
        }
        self.set_active_leaf(target_id)
    }

    pub(crate) fn apply_upgrade(
        &mut self,
        active_leaf_id: NodeId,
        timestamp_str: &str,
    ) -> anyhow::Result<()> {
        if self.is_explicit_tree_version() {
            anyhow::bail!("explicit tree session cannot contain a legacy upgrade marker");
        }
        if self.version.is_none() {
            anyhow::bail!("legacy upgrade marker appears before the session header");
        }
        if self.upgraded {
            anyhow::bail!("session contains more than one legacy upgrade marker");
        }
        if self.active_leaf_id.as_ref() != Some(&active_leaf_id) {
            anyhow::bail!("legacy upgrade marker must target the current virtual leaf");
        }
        if let Some(ts) = parse_timestamp(timestamp_str) {
            self.observe_timestamp(ts);
        }
        self.upgraded = true;
        Ok(())
    }

    pub(crate) fn summary_record(
        &self,
        path: &Path,
        fallback_cwd: &Path,
    ) -> anyhow::Result<SessionIndexRecord> {
        let id = session_id_from_path(path)
            .or_else(|| self.session_id.clone())
            .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))?;
        let (file_size, file_mtime) = super::persistence::session_file_stats(path);
        let mut updated_at = self.updated_at.unwrap_or_default();
        if updated_at == 0 {
            updated_at = file_mtime.map(|mtime| mtime as u64).unwrap_or_default();
        }
        let mut created_at = self
            .created_at
            .or_else(|| super::persistence::timestamp_from_filename(path))
            .unwrap_or(updated_at);
        if created_at == 0 {
            created_at = updated_at;
        }

        let (message_count, first_user_message, last_user_message) =
            if let Some(display) = self.active_display() {
                let first = display
                    .iter()
                    .find_map(|entry| super::persistence::user_message_text(&entry.message));
                let last = display
                    .iter()
                    .rev()
                    .find_map(|entry| super::persistence::user_message_text(&entry.message));
                (display.len() as u64, first, last)
            } else {
                (0, None, None)
            };

        let facts = self.facts();
        Ok(SessionIndexRecord {
            summary: SessionSummary {
                id,
                path: path.to_path_buf(),
                cwd: self
                    .cwd
                    .clone()
                    .unwrap_or_else(|| fallback_cwd.to_path_buf()),
                created_at,
                updated_at,
                message_count,
                title: None,
                first_user_message,
                last_user_message,
            },
            file_size,
            file_mtime,
            node_count: facts.node_count as u64,
            branch_count: facts.branch_count as u64,
            active_leaf_id: facts.active_leaf_id.map(|id| id.to_string()),
            effective_format_version: self.effective_format_version(),
        })
    }

    fn apply_entry(
        &mut self,
        entry: SessionEntry,
        offset: u64,
        legacy_state: &mut PersistedSessionState,
        expected_session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        if let Some(ts) = parse_timestamp(entry.event_timestamp()) {
            self.observe_timestamp(ts);
        }
        match entry {
            SessionEntry::Session {
                version,
                id,
                cwd,
                timestamp,
                ..
            } => {
                validate_session_version(version, Path::new("session"))?;
                if let Some(expected) = expected_session_id {
                    if expected != id {
                        anyhow::bail!(
                            "session header id '{id}' does not match file session id '{expected}'"
                        );
                    }
                }
                if self.session_id.replace(id).is_some() {
                    anyhow::bail!("session log contains more than one session header");
                }
                self.version = Some(version);
                self.cwd = Some(cwd);
                if let Some(created) = parse_timestamp(&timestamp) {
                    self.created_at = Some(created);
                }
            }
            SessionEntry::Node { node } => {
                self.require_explicit_phase("node")?;
                self.insert_explicit_node(node)?;
            }
            SessionEntry::SetLeaf { target_id, .. } => {
                self.require_explicit_phase("set_leaf")?;
                self.set_active_leaf(target_id)?;
            }
            SessionEntry::Upgrade { active_leaf_id, .. } => {
                if self.is_explicit_tree_version() {
                    anyhow::bail!("explicit tree session cannot contain a legacy upgrade marker");
                }
                if self.version.is_none() {
                    anyhow::bail!("legacy upgrade marker appears before the session header");
                }
                if self.upgraded {
                    anyhow::bail!("session contains more than one legacy upgrade marker");
                }
                if self.active_leaf_id.as_ref() != Some(&active_leaf_id) {
                    anyhow::bail!("legacy upgrade marker must target the current virtual leaf");
                }
                self.upgraded = true;
            }
            legacy @ (SessionEntry::Message { .. }
            | SessionEntry::ReplaceHistory { .. }
            | SessionEntry::Snapshot { .. }
            | SessionEntry::SnapshotDelta { .. }) => {
                if self.version.is_none() {
                    anyhow::bail!("legacy record appears before the session header");
                }
                if self.is_explicit_tree_version() || self.upgraded {
                    anyhow::bail!("legacy record appears in the explicit tree phase");
                }
                self.apply_legacy_entry(legacy, offset, legacy_state)?;
            }
        }
        Ok(())
    }

    fn require_explicit_phase(&self, record: &str) -> anyhow::Result<()> {
        match (self.version, self.upgraded) {
            (Some(EXPLICIT_TREE_VERSION..), _) | (Some(1..=3), true) => Ok(()),
            (Some(_), false) => {
                anyhow::bail!("explicit {record} record appears before the legacy upgrade marker")
            }
            (Some(version), true) => {
                anyhow::bail!("unsupported upgraded session version {version}")
            }
            (None, _) => {
                anyhow::bail!("explicit {record} record appears before the session header")
            }
        }
    }

    fn state_from_snapshot(
        &self,
        snapshot: SessionSnapshot,
        display_messages: &[StoredDisplayMessage],
    ) -> anyhow::Result<PersistedSessionState> {
        let session_id = self
            .session_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("state node appears before the session header"))?;
        if snapshot.session_id().as_str() != session_id {
            anyhow::bail!(
                "node snapshot session id '{}' does not match session id '{}'",
                snapshot.session_id(),
                session_id
            );
        }
        Ok(PersistedSessionState {
            model: snapshot.history().to_vec(),
            display: display_messages.to_vec(),
            revision: snapshot.revision(),
            compaction: snapshot.compaction().clone(),
            snapshot: Some(snapshot),
        })
    }

    fn insert_restored_node(
        &mut self,
        node: LoadedNode,
        state: PersistedSessionState,
    ) -> anyhow::Result<()> {
        self.insert_node(node, NodeFacts::from_state(&state))?;
        self.active_state = Some(state);
        self.active_replay = None;
        Ok(())
    }

    fn insert_node(&mut self, node: LoadedNode, facts: NodeFacts) -> anyhow::Result<()> {
        if self.nodes.contains_key(&node.id) {
            anyhow::bail!("duplicate session node id '{}'", node.id);
        }
        if let Some(parent_id) = &node.parent_id {
            if !self.nodes.contains_key(parent_id) {
                anyhow::bail!("node '{}' names missing parent '{parent_id}'", node.id);
            }
            let children = self.children.entry(parent_id.clone()).or_default();
            // A parent crosses from single-child to multi-child exactly once.
            if children.len() == 1 {
                self.branch_count += 1;
            }
            children.push(node.id.clone());
        }
        let id = node.id.clone();
        self.order.push(id.clone());
        self.nodes.insert(
            id.clone(),
            RestoredNode {
                record: node,
                facts,
            },
        );
        self.active_leaf_id = Some(id);
        Ok(())
    }

    fn set_active_leaf(&mut self, target_id: NodeId) -> anyhow::Result<()> {
        if !self.nodes.contains_key(&target_id) {
            anyhow::bail!("active leaf names missing node '{target_id}'");
        }
        if self.active_leaf_id.as_ref() != Some(&target_id) {
            self.active_leaf_id = Some(target_id);
            self.active_state = None;
            self.active_replay = None;
        }
        Ok(())
    }

    fn validate_active_leaf(&self) -> anyhow::Result<()> {
        match (&self.active_leaf_id, &self.active_state) {
            (Some(active), Some(_)) => {
                if !self.nodes.contains_key(active) {
                    anyhow::bail!("active leaf names missing node '{active}'");
                }
                Ok(())
            }
            (None, None) => Ok(()),
            (Some(active), None) => {
                anyhow::bail!("active leaf '{active}' has no materialized state")
            }
            (None, Some(_)) => anyhow::bail!("materialized state exists without an active leaf"),
        }
    }
}

fn snapshot_less_parent_state_changed(
    snapshot: &SessionSnapshot,
    parent: &PersistedSessionState,
) -> bool {
    resume_normalized_history(snapshot.history().to_vec())
        != resume_normalized_history(parent.model.clone())
        || snapshot.compaction() != &parent.compaction
        || snapshot.revision() != parent.revision
}
