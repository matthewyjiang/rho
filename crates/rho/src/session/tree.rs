use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{BufRead, BufReader, Seek},
    path::Path,
};

use rho_sdk::SessionSnapshot;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::persistence::{
    next_revision, session_id_from_path, validate_session_version, PersistedSessionState,
    SessionEntry, StoredDisplayMessage,
};
use super::snapshot_delta::StoredSnapshotDelta;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredCompactionFacts {
    pub(crate) previous_messages: usize,
    pub(crate) current_messages: usize,
    pub(crate) previous_tokens: u64,
    pub(crate) current_tokens: u64,
    pub(crate) cost_usd_micros: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SessionNode {
    pub(crate) id: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_id: Option<NodeId>,
    pub(crate) timestamp: String,
    pub(crate) kind: SessionNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compaction_facts: Option<StoredCompactionFacts>,
    pub(crate) transition: StoredStateTransition,
    #[serde(default)]
    pub(crate) display_messages: Vec<StoredDisplayMessage>,
}

#[derive(Clone, Debug)]
pub(crate) struct RestoredNode {
    node: SessionNode,
    state: PersistedSessionState,
}

impl RestoredNode {
    pub(crate) fn id(&self) -> &NodeId {
        &self.node.id
    }

    pub(crate) fn parent_id(&self) -> Option<&NodeId> {
        self.node.parent_id.as_ref()
    }

    pub(crate) fn kind(&self) -> SessionNodeKind {
        self.node.kind
    }

    #[cfg(test)]
    pub(crate) fn timestamp(&self) -> &str {
        &self.node.timestamp
    }

    pub(crate) fn display_messages(&self) -> &[StoredDisplayMessage] {
        &self.node.display_messages
    }

    pub(crate) fn state(&self) -> &PersistedSessionState {
        &self.state
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
    version: Option<u32>,
    upgraded: bool,
}

impl SessionTree {
    pub(crate) fn load(path: &Path) -> anyhow::Result<Self> {
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
            tree.apply_entry(
                entry,
                offset,
                &mut legacy_state,
                expected_session_id.as_deref(),
            )?;
        }
        tree.validate_active_leaf()?;
        Ok(tree)
    }

    pub(crate) fn active_leaf_id(&self) -> Option<&NodeId> {
        self.active_leaf_id.as_ref()
    }

    pub(crate) fn active_state(&self) -> Option<&PersistedSessionState> {
        self.active_leaf_id
            .as_ref()
            .and_then(|id| self.nodes.get(id))
            .map(RestoredNode::state)
    }

    pub(crate) fn node(&self, id: &NodeId) -> Option<&RestoredNode> {
        self.nodes.get(id)
    }

    #[cfg(test)]
    pub(crate) fn children(&self, id: &NodeId) -> &[NodeId] {
        self.children.get(id).map_or(&[], Vec::as_slice)
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
                let detail = if let Some(facts) = node.node.compaction_facts.as_ref() {
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
                    let compaction = &node.state.compaction;
                    match (
                        compaction.last_previous_tokens(),
                        compaction.last_current_tokens(),
                    ) {
                        (Some(previous), Some(current)) => {
                            format!("\n{previous} → {current} estimated tokens")
                        }
                        _ => String::new(),
                    }
                };
                display.push(StoredDisplayMessage {
                    timestamp: node.node.timestamp.clone(),
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
            branch_count: self
                .children
                .values()
                .filter(|children| children.len() > 1)
                .count(),
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
            if continuation_is_valid(&node.state.model) {
                let kind = match node.kind() {
                    SessionNodeKind::Commit => SessionTreeItemKind::Turn,
                    SessionNodeKind::Compaction => SessionTreeItemKind::Compaction,
                };
                let first_user_text = (kind == SessionTreeItemKind::Turn).then(|| {
                    node.display_messages()
                        .iter()
                        .find_map(|stored| super::persistence::user_message_text(&stored.message))
                        .unwrap_or_else(|| "completed turn".into())
                });
                let compaction_facts = (kind == SessionTreeItemKind::Compaction).then(|| {
                    node.node
                        .compaction_facts
                        .clone()
                        .unwrap_or_else(|| StoredCompactionFacts {
                            previous_messages: node
                                .parent_id()
                                .and_then(|id| self.node(id))
                                .map_or(0, |parent| parent.state.model.len()),
                            current_messages: node.state.model.len(),
                            previous_tokens: node
                                .state
                                .compaction
                                .last_previous_tokens()
                                .unwrap_or_default(),
                            current_tokens: node
                                .state
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
            }
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
        if self.upgraded || self.version == Some(4) {
            4
        } else {
            self.version.unwrap_or(4)
        }
    }

    pub(crate) fn needs_upgrade_marker(&self) -> bool {
        self.version.is_some_and(|version| version < 4) && !self.upgraded
    }

    fn apply_entry(
        &mut self,
        entry: SessionEntry,
        offset: u64,
        legacy_state: &mut PersistedSessionState,
        expected_session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        match entry {
            SessionEntry::Session { version, id, .. } => {
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
                if self.version == Some(4) {
                    anyhow::bail!("version 4 session cannot contain a legacy upgrade marker");
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
            legacy => {
                if self.version.is_none() {
                    anyhow::bail!("legacy record appears before the session header");
                }
                if self.version == Some(4) || self.upgraded {
                    anyhow::bail!("legacy record appears in the explicit tree phase");
                }
                self.apply_legacy_entry(legacy, offset, legacy_state)?;
            }
        }
        Ok(())
    }

    fn require_explicit_phase(&self, record: &str) -> anyhow::Result<()> {
        match (self.version, self.upgraded) {
            (Some(4), _) | (Some(1..=3), true) => Ok(()),
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

    fn apply_legacy_entry(
        &mut self,
        entry: SessionEntry,
        offset: u64,
        state: &mut PersistedSessionState,
    ) -> anyhow::Result<()> {
        let previous = state.clone();
        let (timestamp, display_messages, suggested_kind, changed) = match entry {
            SessionEntry::Message {
                timestamp,
                message,
                display_message,
            } => {
                let display = StoredDisplayMessage {
                    timestamp: timestamp.clone(),
                    message: display_message.map_or_else(|| message.clone(), |message| *message),
                };
                state.model.push(message);
                state.display.push(display.clone());
                state.revision = next_revision(state.revision)?;
                refresh_snapshot_state(state);
                (timestamp, vec![display], SessionNodeKind::Commit, true)
            }
            SessionEntry::ReplaceHistory {
                timestamp,
                messages,
            } => {
                super::persistence::apply_legacy_history_replacement(state, messages)?;
                refresh_snapshot_state(state);
                (timestamp, Vec::new(), SessionNodeKind::Compaction, true)
            }
            SessionEntry::Snapshot {
                timestamp,
                snapshot,
                display_messages,
            } => {
                let replaced_history = !state.model.is_empty()
                    && !snapshot.history().starts_with(state.model.as_slice());
                let compaction = snapshot.compaction() != &state.compaction || replaced_history;
                let changed = state.snapshot.is_none()
                    || snapshot.revision() != state.revision
                    || snapshot.history() != state.model
                    || snapshot.compaction() != &state.compaction;
                state.model = snapshot.history().to_vec();
                state.display.extend(display_messages.clone());
                state.revision = snapshot.revision();
                state.compaction = snapshot.compaction().clone();
                state.snapshot = Some(*snapshot);
                (
                    timestamp,
                    display_messages,
                    if compaction {
                        SessionNodeKind::Compaction
                    } else {
                        SessionNodeKind::Commit
                    },
                    changed,
                )
            }
            SessionEntry::SnapshotDelta {
                timestamp,
                delta,
                display_messages,
            } => {
                let base = state.snapshot.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("snapshot delta does not have a complete base snapshot")
                })?;
                let snapshot = delta.restore(base)?;
                let changed = snapshot.revision() != state.revision
                    || snapshot.history() != state.model
                    || snapshot.compaction() != &state.compaction;
                let kind = if snapshot.compaction() != &state.compaction {
                    SessionNodeKind::Compaction
                } else {
                    SessionNodeKind::Commit
                };
                state.model = snapshot.history().to_vec();
                state.display.extend(display_messages.clone());
                state.revision = snapshot.revision();
                state.compaction = snapshot.compaction().clone();
                state.snapshot = Some(snapshot);
                (timestamp, display_messages, kind, changed)
            }
            SessionEntry::Session { .. }
            | SessionEntry::Node { .. }
            | SessionEntry::SetLeaf { .. }
            | SessionEntry::Upgrade { .. } => {
                unreachable!("entry was classified before projection")
            }
        };

        if !changed {
            if let Some(active) = self.active_leaf_id.as_ref() {
                if let Some(node) = self.nodes.get_mut(active) {
                    node.node.display_messages.extend(display_messages);
                    node.state.display = state.display.clone();
                }
            }
            return Ok(());
        }

        let id = NodeId::legacy(offset);
        let parent_id = self.active_leaf_id.clone();
        let snapshot = state.snapshot.clone().or(previous.snapshot).ok_or_else(|| {
            anyhow::anyhow!("legacy state at byte offset {offset} has no snapshot transition")
        });
        let transition = match snapshot {
            Ok(snapshot) => StoredStateTransition::Snapshot {
                snapshot: Box::new(snapshot),
            },
            Err(_) => {
                // Message-only v1 records predate snapshots. Their restored state is retained
                // directly on the virtual node and the transition is never serialized.
                StoredStateTransition::Snapshot {
                    snapshot: Box::new(synthetic_snapshot(self.session_id.as_deref(), state)?),
                }
            }
        };
        let node = SessionNode {
            id,
            parent_id,
            timestamp,
            kind: suggested_kind,
            compaction_facts: None,
            transition,
            display_messages,
        };
        self.insert_restored_node(node, state.clone())
    }

    fn insert_explicit_node(&mut self, node: SessionNode) -> anyhow::Result<()> {
        if node.kind == SessionNodeKind::Commit && node.compaction_facts.is_some() {
            anyhow::bail!("commit node '{}' cannot store compaction facts", node.id);
        }
        if node.parent_id.is_none() && !self.nodes.is_empty() {
            anyhow::bail!("session node '{}' creates a disconnected root", node.id);
        }
        let state = match (&node.parent_id, &node.transition) {
            (None, StoredStateTransition::Snapshot { snapshot }) => {
                if node.kind == SessionNodeKind::Compaction {
                    anyhow::bail!("root node '{}' cannot be a compaction", node.id);
                }
                self.state_from_snapshot(snapshot.as_ref(), &node.display_messages)?
            }
            (None, StoredStateTransition::SnapshotDelta { .. }) => {
                anyhow::bail!("root node '{}' cannot store a snapshot delta", node.id)
            }
            (Some(parent_id), transition) => {
                let parent = self.nodes.get(parent_id).ok_or_else(|| {
                    anyhow::anyhow!("node '{}' names missing parent '{parent_id}'", node.id)
                })?;
                let parent_snapshot = parent.state.snapshot.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("parent '{parent_id}' has no complete snapshot")
                })?;
                if node.kind == SessionNodeKind::Compaction
                    && !matches!(transition, StoredStateTransition::Snapshot { .. })
                {
                    anyhow::bail!("compaction node '{}' must store a full snapshot", node.id);
                }
                let snapshot = match transition {
                    StoredStateTransition::Snapshot { snapshot } => snapshot.as_ref().clone(),
                    StoredStateTransition::SnapshotDelta { delta } => {
                        delta.restore(parent_snapshot)?
                    }
                };
                if snapshot != *parent_snapshot && snapshot.revision() <= parent.state.revision {
                    anyhow::bail!(
                        "node '{}' changed state without advancing parent revision {}",
                        node.id,
                        parent.state.revision
                    );
                }
                let compaction_changed = snapshot.compaction() != &parent.state.compaction;
                if compaction_changed != (node.kind == SessionNodeKind::Compaction) {
                    anyhow::bail!(
                        "node '{}' kind does not match its compaction state transition",
                        node.id
                    );
                }
                let mut state = self.state_from_snapshot(&snapshot, &[])?;
                state.display = parent.state.display.clone();
                state.display.extend(node.display_messages.clone());
                state
            }
        };
        self.insert_restored_node(node, state)
    }

    fn state_from_snapshot(
        &self,
        snapshot: &SessionSnapshot,
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
            snapshot: Some(snapshot.clone()),
            revision: snapshot.revision(),
            compaction: snapshot.compaction().clone(),
        })
    }

    fn insert_restored_node(
        &mut self,
        node: SessionNode,
        state: PersistedSessionState,
    ) -> anyhow::Result<()> {
        if self.nodes.contains_key(&node.id) {
            anyhow::bail!("duplicate session node id '{}'", node.id);
        }
        if let Some(parent_id) = &node.parent_id {
            if !self.nodes.contains_key(parent_id) {
                anyhow::bail!("node '{}' names missing parent '{parent_id}'", node.id);
            }
            self.children
                .entry(parent_id.clone())
                .or_default()
                .push(node.id.clone());
        }
        let id = node.id.clone();
        self.order.push(id.clone());
        self.nodes.insert(id.clone(), RestoredNode { node, state });
        self.active_leaf_id = Some(id);
        Ok(())
    }

    fn set_active_leaf(&mut self, target_id: NodeId) -> anyhow::Result<()> {
        if !self.nodes.contains_key(&target_id) {
            anyhow::bail!("active leaf names missing node '{target_id}'");
        }
        self.active_leaf_id = Some(target_id);
        Ok(())
    }

    fn validate_active_leaf(&self) -> anyhow::Result<()> {
        if let Some(active) = &self.active_leaf_id {
            if !self.nodes.contains_key(active) {
                anyhow::bail!("active leaf names missing node '{active}'");
            }
        }
        Ok(())
    }
}

fn continuation_is_valid(messages: &[rho_providers::model::Message]) -> bool {
    super::drop_incomplete_tool_turn_tail(messages.to_vec()).len() == messages.len()
}

fn refresh_snapshot_state(state: &mut PersistedSessionState) {
    let Some(previous) = state.snapshot.take() else {
        return;
    };
    let mut snapshot = SessionSnapshot::new(
        previous.session_id().clone(),
        state.revision,
        state.model.clone(),
        previous.provider().clone(),
        state.compaction.clone(),
    );
    for (key, value) in previous.metadata() {
        snapshot = snapshot.with_metadata(key.clone(), value.clone());
    }
    if let Some(prompt_cache_key) = previous.prompt_cache_key() {
        snapshot = snapshot.with_prompt_cache_key(prompt_cache_key);
    }
    state.snapshot = Some(snapshot);
}

fn synthetic_snapshot(
    session_id: Option<&str>,
    state: &PersistedSessionState,
) -> anyhow::Result<SessionSnapshot> {
    use rho_providers::model::ModelIdentity;
    use rho_sdk::SessionId;

    let id = session_id.ok_or_else(|| anyhow::anyhow!("legacy record appears before header"))?;
    Ok(SessionSnapshot::new(
        SessionId::from_string(id.to_owned())?,
        state.revision,
        state.model.clone(),
        ModelIdentity::new("legacy", "legacy", "legacy"),
        state.compaction.clone(),
    ))
}
