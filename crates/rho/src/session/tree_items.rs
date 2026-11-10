//! The session tree as `/tree` lists it.

use std::collections::HashSet;

use super::{NodeId, RestoredNode, SessionNodeKind, SessionTree, StoredCompactionFacts};

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

impl SessionTree {
    /// The node after `node`, when exactly one follows it.
    fn only_child(&self, node: &RestoredNode) -> Option<&RestoredNode> {
        match self.children.get(node.id()).map(Vec::as_slice) {
            Some([only]) => self.node(only),
            _ => None,
        }
    }

    /// Whether `node` carries on the turn of the node before it: a commit
    /// that shows no user message.
    fn continues_turn(node: &RestoredNode) -> bool {
        node.kind() == SessionNodeKind::Commit
            && !node.display_messages().iter().any(|stored| {
                // Any prompt, an image alone included.
                matches!(
                    stored.message.semantic(),
                    rho_sdk::model::SemanticMessage::User(_)
                )
            })
    }

    /// The prompt of the turn `node` belongs to: the first user text at
    /// `node` or before it, whatever the turn saved after its prompt.
    pub(crate) fn turn_prompt(&self, node: &NodeId) -> Option<String> {
        let mut at = self.node(node);
        while let Some(node) = at {
            let prompt = node
                .display_messages()
                .iter()
                .find_map(|stored| super::super::persistence::user_message_text(&stored.message));
            if prompt.is_some() {
                return prompt;
            }
            at = node.parent_id().and_then(|parent| self.node(parent));
        }
        None
    }

    /// The tree as `/tree` lists it: one row for each prompt and all that
    /// followed it. A node that adds no user message joins the row of the
    /// node before it when nothing branches between them, and the row
    /// names the last node of its run, the conversation once the turn
    /// ended, since some hosts save a turn as several nodes, one for each
    /// step.
    pub(crate) fn turn_items(&self) -> anyhow::Result<Vec<SessionTreeItem>> {
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
                // Classify from durable state, not missing user text: a root
                // that shows nothing is the session start.
                SessionNodeKind::Commit
                    if node.parent_id().is_none()
                        // Whatever the model holds: a host that saves the
                        // session before its first prompt saves the system
                        // prompt with it.
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
                    .find_map(|stored| {
                        super::super::persistence::user_message_text(&stored.message)
                    })
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
            // The last node of the row's run, and whether the run holds
            // the active leaf.
            let mut last = node;
            let mut active = self.active_leaf_id() == Some(node.id());
            while let Some(next) = self
                .only_child(last)
                .filter(|next| Self::continues_turn(next))
            {
                last = next;
                active |= self.active_leaf_id() == Some(last.id());
            }
            let id = last.id().clone();
            traversal.push(SessionTreeItem {
                id: id.clone(),
                depth,
                kind,
                first_user_text,
                compaction_facts,
                active,
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
}
