use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    InputName, InputSchema, Node, NodeId, OutputReference, OutputSchema, WorkflowError,
    WorkflowName, WorkflowResult, WorkflowValue,
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
    pub(crate) exports: BTreeMap<ExportName, OutputReference>,
}

/// An export label is any nonempty string, not a task identifier.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct ExportName(String);

impl TryFrom<String> for ExportName {
    type Error = WorkflowError;

    fn try_from(name: String) -> WorkflowResult<Self> {
        if name.is_empty() {
            return Err(WorkflowError::Schema {
                path: "scope.exports".to_owned(),
                reason: "export name must not be empty".to_owned(),
            });
        }
        Ok(Self(name))
    }
}

impl From<ExportName> for String {
    fn from(name: ExportName) -> Self {
        name.0
    }
}

impl std::fmt::Display for ExportName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl WorkflowProgram {
    pub(crate) fn scope(&self, id: ScopeDefinitionId) -> &ScopeDefinition {
        match id {
            ScopeDefinitionId::Root => &self.root,
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
            self.export_schema(name, reference)?.validate_definition()?;
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
        name: &ExportName,
        reference: &OutputReference,
    ) -> WorkflowResult<&OutputSchema> {
        let error = |reason: &str| WorkflowError::Schema {
            path: format!("scope.exports.{name}"),
            reason: reason.to_owned(),
        };
        let node = self
            .nodes
            .get(&reference.node)
            .ok_or_else(|| error("export references an unknown node"))?;
        let schema = node
            .output_schema()
            .ok_or_else(|| error("export source node has no output schema"))?;
        schema
            .schema_at_path(&reference.path.0)
            .ok_or_else(|| error("export path does not exist in the source output schema"))
    }

    /// Resolve required bindings without treating an absent value as explicit null.
    /// `None` means a source node or selected field has no value. Invalid contracts
    /// and values remain errors rather than being treated as missing outputs.
    /// Alias expansion is bounded at plan time by validate_export_budget using
    /// the retained-output capacity and each source's enforced output bound.
    pub(crate) fn resolve_exports(
        &self,
        outputs: &BTreeMap<NodeId, WorkflowValue>,
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
                    resolved.insert(name.0.as_str(), value);
                }
                None => missing = true,
            }
        }
        if missing {
            return Ok(None);
        }
        Ok(Some(
            resolved
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.clone()))
                .collect(),
        ))
    }
}
