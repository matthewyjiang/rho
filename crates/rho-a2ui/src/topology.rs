//! The checks over a surface's whole component graph: composition
//! constraints, then root presence, dangling references, cycles, depth and
//! reachability. They run on the candidate graph an update would leave, so a
//! rejected update changes nothing.

use std::collections::{HashMap, HashSet};

use crate::catalog::ComponentDef;
use crate::component::{ChildEdge, Component};
use crate::message::{ErrorCode, ErrorMessage};
use crate::processor::ValidationConfig;

/// The id of the component that mounts in the surface's `Surface` container.
pub(crate) const ROOT: &str = "root";

/// The component type the protocol reserves for the surface container.
const SURFACE: &str = "Surface";

/// The deepest a component may sit below the root, and the deepest a value
/// may nest, from the A2UI core blueprint.
pub(crate) const MAX_NESTING: usize = 50;

/// One component of the candidate graph.
pub(crate) struct Node<'a> {
    pub(crate) component: &'a Component,
    /// Its definition, or `None` for a type its catalog lacks.
    pub(crate) def: Option<&'a ComponentDef>,
    /// Its position in the message being checked, if it is in it.
    pub(crate) index: Option<usize>,
}

/// Checks the candidate graph `nodes`, in surface order, of `surface`.
pub(crate) fn check(
    nodes: &[Node<'_>],
    config: ValidationConfig,
    surface: &str,
) -> Result<(), ErrorMessage> {
    let graph = Graph::new(nodes);
    graph.composition(surface)?;
    graph.topology(config, surface)
}

struct Graph<'a, 'n> {
    nodes: &'n [Node<'a>],
    by_id: HashMap<&'a str, &'n Node<'a>>,
    edges: Vec<Vec<ChildEdge>>,
}

impl<'a, 'n> Graph<'a, 'n> {
    fn new(nodes: &'n [Node<'a>]) -> Self {
        Self {
            nodes,
            by_id: nodes
                .iter()
                .map(|node| (node.component.id(), node))
                .collect(),
            edges: nodes
                .iter()
                .map(|node| node.component.child_edges())
                .collect(),
        }
    }

    /// `allowedParents` and `allowedChildren`, with `Surface` the parent of
    /// the root only.
    fn composition(&self, surface: &str) -> Result<(), ErrorMessage> {
        if let Some(root) = self.by_id.get(ROOT) {
            check_parent(root, SURFACE, SURFACE, None, surface)?;
        }
        for (parent, edges) in self.nodes.iter().zip(&self.edges) {
            for edge in edges {
                let Some(child) = self.by_id.get(edge.target.as_str()) else {
                    continue;
                };
                check_parent(
                    child,
                    parent.component.id(),
                    parent.component.kind(),
                    Some(parent),
                    surface,
                )?;
                let allowed = parent.def.and_then(ComponentDef::allowed_children);
                if let Some(allowed) = allowed
                    .filter(|allowed| !allowed.iter().any(|name| name == child.component.kind()))
                {
                    let at = parent
                        .index
                        .map(|index| format!("/components/{index}{}", edge.pointer))
                        .or_else(|| child.index.map(|index| format!("/components/{index}")))
                        .unwrap_or_else(|| "/components".to_owned());
                    return Err(ErrorMessage::at(
                        ErrorCode::UnallowedChild,
                        surface,
                        &at,
                        format!(
                            "Container '{}' ({}) cannot contain child '{}' ({}). Allowed children: {}.",
                            parent.component.id(),
                            parent.component.kind(),
                            child.component.id(),
                            child.component.kind(),
                            names(allowed),
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn topology(&self, config: ValidationConfig, surface: &str) -> Result<(), ErrorMessage> {
        let integrity =
            |message: String| ErrorMessage::surface(ErrorCode::IntegrityError, surface, message);
        let recursion = |message: String| {
            ErrorMessage::surface(ErrorCode::RecursionLimitExceeded, surface, message)
        };
        for (node, edges) in self.nodes.iter().zip(&self.edges) {
            let id = node.component.id();
            if edges.iter().any(|edge| edge.target == id) {
                return Err(recursion(format!(
                    "Self-reference detected: component '{id}' references itself"
                )));
            }
        }
        let has_root = self.by_id.contains_key(ROOT);
        if !has_root && !config.allow_missing_root {
            return Err(integrity(format!(
                "Missing root component: no component has id '{ROOT}'"
            )));
        }
        if !config.allow_dangling_references {
            for (node, edges) in self.nodes.iter().zip(&self.edges) {
                if let Some(edge) = edges
                    .iter()
                    .find(|edge| !self.by_id.contains_key(edge.target.as_str()))
                {
                    return Err(integrity(format!(
                        "Dangling reference: component '{}' references non-existent component '{}' in field '{}'",
                        node.component.id(),
                        edge.target,
                        edge.field,
                    )));
                }
            }
        }
        if !has_root {
            return Ok(());
        }
        let reachable = self.walk().map_err(recursion)?;
        if !config.allow_orphans {
            if let Some(orphan) = self
                .nodes
                .iter()
                .find(|node| !reachable.contains(node.component.id()))
            {
                return Err(integrity(format!(
                    "Component '{}' is not reachable from '{ROOT}'",
                    orphan.component.id()
                )));
            }
        }
        Ok(())
    }

    /// Walks the graph depth first from the root and returns what it reaches,
    /// or why it stopped: a cycle, or a component deeper than
    /// [`MAX_NESTING`].
    fn walk(&self) -> Result<HashSet<&'a str>, String> {
        let mut walk = Walk {
            graph: self,
            index: self
                .nodes
                .iter()
                .enumerate()
                .map(|(at, node)| (node.component.id(), at))
                .collect(),
            path: Vec::new(),
            explored: HashMap::new(),
        };
        walk.visit(ROOT)?;
        Ok(walk.explored.into_keys().collect())
    }
}

struct Walk<'a, 'n, 'g> {
    graph: &'g Graph<'a, 'n>,
    index: HashMap<&'a str, usize>,
    /// The components from the root down to the one being visited.
    path: Vec<&'a str>,
    /// The depth each reached component was explored at, the deepest so
    /// far. Exploring it again no deeper finds nothing new.
    explored: HashMap<&'a str, usize>,
}

impl Walk<'_, '_, '_> {
    fn visit(&mut self, id: &str) -> Result<(), String> {
        let Some(&at) = self.index.get(id) else {
            return Ok(());
        };
        let own = self.graph.nodes[at].component.id();
        if self.path.contains(&own) {
            self.path.push(own);
            return Err(format!(
                "Circular reference detected: {}",
                self.path.join(" -> ")
            ));
        }
        let depth = self.path.len();
        if depth > MAX_NESTING {
            return Err(format!(
                "Global recursion limit exceeded: component '{own}' is nested deeper than {MAX_NESTING} levels"
            ));
        }
        if self.explored.get(own).is_some_and(|&seen| seen >= depth) {
            return Ok(());
        }
        self.explored.insert(own, depth);
        self.path.push(own);
        for edge in &self.graph.edges[at] {
            self.visit(&edge.target)?;
        }
        self.path.pop();
        Ok(())
    }
}

/// Fails when `child` may not be placed under a parent of type `parent_type`.
fn check_parent(
    child: &Node<'_>,
    parent_id: &str,
    parent_type: &str,
    parent: Option<&Node<'_>>,
    surface: &str,
) -> Result<(), ErrorMessage> {
    let Some(allowed) = child.def.and_then(ComponentDef::allowed_parents) else {
        return Ok(());
    };
    if allowed.iter().any(|name| name == parent_type) {
        return Ok(());
    }
    let at = child
        .index
        .or_else(|| parent.and_then(|parent| parent.index))
        .map_or_else(
            || "/components".to_owned(),
            |index| format!("/components/{index}"),
        );
    Err(ErrorMessage::at(
        ErrorCode::UnallowedParent,
        surface,
        &at,
        format!(
            "Component '{}' ({}) cannot be placed under parent '{parent_id}' ({parent_type}). Allowed parents: {}.",
            child.component.id(),
            child.component.kind(),
            names(allowed),
        ),
    ))
}

/// A list of names as the protocol's messages write it: `['Text', 'Button']`.
fn names(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
    format!("[{}]", quoted.join(", "))
}
