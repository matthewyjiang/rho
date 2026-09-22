use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    InputName, InputSchema, Node, NodeId, OutputReference, OutputSchema, WorkflowError,
    WorkflowGraph, WorkflowName, WorkflowResult, WorkflowValue,
};

/// The executable definition produced by the compiler. This format currently
/// supports one root scope of leaf tasks, not map or iteration controllers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowProgram {
    pub(crate) name: WorkflowName,
    pub(crate) root: ScopeDefinition,
}

/// Identifies a frozen scope body, independently of any runtime invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScopeDefinitionId {
    Root,
}

/// Dependency and output references are local to this scope. Root parameters
/// retain their source types even though build(inputs) specializes leaf inputs
/// at compile time. Runtime state is stored separately in WorkflowState.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeDefinition {
    pub(crate) parameters: BTreeMap<InputName, InputSchema>,
    pub(crate) nodes: BTreeMap<NodeId, Node>,
    pub(crate) exports: BTreeMap<String, OutputReference>,
}

impl WorkflowProgram {
    pub(crate) fn scope_definition(&self, definition: ScopeDefinitionId) -> &ScopeDefinition {
        match definition {
            ScopeDefinitionId::Root => &self.root,
        }
    }

    /// Lower the source-only graph into the executable root scope.
    pub(crate) fn lower(
        graph: WorkflowGraph,
        parameters: BTreeMap<InputName, InputSchema>,
    ) -> Self {
        Self {
            name: graph.name,
            root: ScopeDefinition {
                parameters,
                nodes: graph.nodes,
                exports: BTreeMap::new(),
            },
        }
    }

    pub(crate) fn validate_bindings(
        &self,
        inputs: &BTreeMap<InputName, WorkflowValue>,
    ) -> WorkflowResult<()> {
        if self.root.parameters.keys().ne(inputs.keys()) {
            return Err(WorkflowError::Schema {
                path: "program.root.parameters".to_owned(),
                reason: "root binding names must exactly match declared parameters".to_owned(),
            });
        }
        for (name, schema) in &self.root.parameters {
            if !schema.validate(&inputs[name]) {
                return Err(WorkflowError::Schema {
                    path: format!("inputs.{name}"),
                    reason: "root binding does not match its parameter type".to_owned(),
                });
            }
        }
        Ok(())
    }
}

impl ScopeDefinition {
    pub(crate) fn validate_exports(&self) -> WorkflowResult<()> {
        for (name, reference) in &self.exports {
            self.export_schema(name, reference)?;
        }
        Ok(())
    }

    /// Reserve the serialized export map's worst case from each source's frozen
    /// output bound. Reject amplification before any task can execute.
    pub(crate) fn validate_export_budget(&self, limit: u64) -> WorkflowResult<()> {
        let mut bound = 2_u64; // The empty JSON object's opening and closing braces.
        for (index, (name, reference)) in self.exports.iter().enumerate() {
            let node = self
                .nodes
                .get(&reference.node)
                .ok_or_else(|| WorkflowError::Schema {
                    path: format!("scope.exports.{name}"),
                    reason: "export references an unknown node".to_owned(),
                })?;
            let key_bytes = serde_json::to_vec(name)?.len() as u64;
            // Serialized key, colon, value, and a comma after the first entry.
            for bytes in [key_bytes, 1, node.max_output_bytes, u64::from(index != 0)] {
                bound = bound
                    .checked_add(bytes)
                    .ok_or(WorkflowError::BudgetExceeded {
                        budget: "scope export bytes",
                        limit,
                        actual: u64::MAX,
                    })?;
            }
        }
        if bound > limit {
            return Err(WorkflowError::BudgetExceeded {
                budget: "scope export bytes",
                limit,
                actual: bound,
            });
        }
        Ok(())
    }

    fn export_schema(
        &self,
        name: &str,
        reference: &OutputReference,
    ) -> WorkflowResult<&OutputSchema> {
        let error = |reason: &str| WorkflowError::Schema {
            path: format!("scope.exports.{name}"),
            reason: reason.to_owned(),
        };
        if name.is_empty() {
            return Err(error("object field name must not be empty"));
        }
        let node = self
            .nodes
            .get(&reference.node)
            .ok_or_else(|| error("export references an unknown node"))?;
        let schema = node
            .output_schema()
            .ok_or_else(|| error("export source node has no output schema"))?;
        schema.validate_definition()?;
        schema
            .schema_at_path(&reference.path.0)
            .ok_or_else(|| error("export path does not exist in the source output schema"))
    }

    /// Resolve required bindings without treating an absent value as explicit null.
    /// `None` means a source node or selected field has no value. Invalid contracts
    /// and values remain errors rather than being treated as missing outputs.
    /// Check the serialized export map before cloning values: aliases can expand
    /// a small source output into a much larger scope result. Counting stops at
    /// the first write over budget, before cloning or serializing further aliases.
    pub(crate) fn resolve_exports(
        &self,
        outputs: &BTreeMap<NodeId, WorkflowValue>,
        scope_export_bytes_limit: u64,
    ) -> WorkflowResult<Option<BTreeMap<String, WorkflowValue>>> {
        let mut resolved = BTreeMap::new();
        let mut missing = false;
        for (name, reference) in &self.exports {
            let schema = self.export_schema(name, reference)?;
            match outputs
                .get(&reference.node)
                .and_then(|value| value.at_path(&reference.path.0))
            {
                Some(value) => {
                    schema.validate_value(value)?;
                    resolved.insert(name.as_str(), value);
                }
                None => missing = true,
            }
        }
        if missing {
            return Ok(None);
        }
        let mut bytes = SerializedBytes {
            observed: 0,
            limit: scope_export_bytes_limit,
        };
        let serialized = serde_json::to_writer(&mut bytes, &resolved);
        if bytes.observed > bytes.limit {
            return Err(WorkflowError::BudgetExceeded {
                budget: "scope export bytes",
                limit: bytes.limit,
                actual: bytes.observed,
            });
        }
        serialized?;
        Ok(Some(
            resolved
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.clone()))
                .collect(),
        ))
    }
}

/// Count JSON keys, values, and punctuation without retaining bytes. Abort once
/// the measured prefix exceeds the export-map budget, bounding alias expansion.
struct SerializedBytes {
    observed: u64,
    limit: u64,
}

impl std::io::Write for SerializedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.observed = self
            .observed
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| {
                std::io::Error::other("scope export bytes exceed the representable u64 size")
            })?;
        if self.observed > self.limit {
            return Err(std::io::Error::other("scope export bytes budget exceeded"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
