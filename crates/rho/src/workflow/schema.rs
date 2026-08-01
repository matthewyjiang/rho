use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{WorkflowError, WorkflowResult, WorkflowValue};

// Receipt: limit_receipt.json planning.accepted.schema_depth. This matches the
// planning budget, so stored data cannot bypass the planning contract.
const OUTPUT_SCHEMA_DEPTH_LIMIT: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OutputSchema {
    Null,
    Bool,
    Integer,
    String,
    Enum {
        members: BTreeSet<WorkflowValue>,
    },
    List {
        item: Box<OutputSchema>,
    },
    Object {
        fields: BTreeMap<String, ObjectFieldSchema>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObjectFieldSchema {
    pub(crate) schema: OutputSchema,
    pub(crate) required: bool,
}

impl OutputSchema {
    pub(crate) fn depth(&self) -> usize {
        let mut maximum = 0;
        let mut pending = vec![(self, 1)];
        while let Some((schema, depth)) = pending.pop() {
            maximum = maximum.max(depth);
            match schema {
                Self::List { item } => pending.push((item, depth + 1)),
                Self::Object { fields } => {
                    pending.extend(fields.values().map(|field| (&field.schema, depth + 1)))
                }
                Self::Null | Self::Bool | Self::Integer | Self::String | Self::Enum { .. } => {}
            }
        }
        maximum
    }

    pub(crate) fn validate_definition(&self) -> WorkflowResult<()> {
        let actual = self.depth() as u64;
        let limit = OUTPUT_SCHEMA_DEPTH_LIMIT as u64;
        if actual > limit {
            return Err(WorkflowError::BudgetExceeded {
                budget: "output schema depth",
                limit,
                actual,
            });
        }
        self.validate_definition_at("$")
    }

    fn validate_definition_at(&self, path: &str) -> WorkflowResult<()> {
        match self {
            Self::Enum { members } => {
                if members.is_empty() {
                    return Err(WorkflowError::Schema {
                        path: path.to_owned(),
                        reason: "enum must have at least one member".to_owned(),
                    });
                }
                if let Some(value) = members.iter().find(|value| !value.scalar()) {
                    return Err(WorkflowError::Schema {
                        path: path.to_owned(),
                        reason: format!("enum member must be scalar, got {}", value.kind()),
                    });
                }
            }
            Self::List { item } => item.validate_definition_at(&format!("{path}[]"))?,
            Self::Object { fields } => {
                for (name, field) in fields {
                    if name.is_empty() {
                        return Err(WorkflowError::Schema {
                            path: path.to_owned(),
                            reason: "object field name must not be empty".to_owned(),
                        });
                    }
                    field
                        .schema
                        .validate_definition_at(&format!("{path}.{name}"))?;
                }
            }
            Self::Null | Self::Bool | Self::Integer | Self::String => {}
        }
        Ok(())
    }

    pub(crate) fn validate_value(&self, value: &WorkflowValue) -> WorkflowResult<()> {
        self.validate_value_at(value, "$", 1, self.depth())
    }

    fn validate_value_at(
        &self,
        value: &WorkflowValue,
        path: &str,
        depth: usize,
        maximum_depth: usize,
    ) -> WorkflowResult<()> {
        if depth > maximum_depth {
            return Err(WorkflowError::Schema {
                path: path.to_owned(),
                reason: format!("value exceeds schema depth {maximum_depth}"),
            });
        }
        let type_matches = matches!(
            (self, value),
            (Self::Null, WorkflowValue::Null)
                | (Self::Bool, WorkflowValue::Bool(_))
                | (Self::Integer, WorkflowValue::Integer(_))
                | (Self::String, WorkflowValue::String(_))
        );
        if type_matches {
            return Ok(());
        }
        match (self, value) {
            (Self::Enum { members }, value) if members.contains(value) => Ok(()),
            (Self::Enum { .. }, value) => Err(schema_mismatch(path, "enum member", value.kind())),
            (Self::List { item }, WorkflowValue::List(values)) => {
                values.iter().enumerate().try_for_each(|(index, value)| {
                    item.validate_value_at(
                        value,
                        &format!("{path}[{index}]"),
                        depth + 1,
                        maximum_depth,
                    )
                })
            }
            (Self::Object { fields }, WorkflowValue::Object(values)) => {
                if let Some(name) = values.keys().find(|name| !fields.contains_key(*name)) {
                    return Err(WorkflowError::Schema {
                        path: format!("{path}.{name}"),
                        reason: "unknown object field".to_owned(),
                    });
                }
                for (name, field) in fields {
                    match values.get(name) {
                        Some(value) => field.schema.validate_value_at(
                            value,
                            &format!("{path}.{name}"),
                            depth + 1,
                            maximum_depth,
                        )?,
                        None if field.required => {
                            return Err(WorkflowError::Schema {
                                path: format!("{path}.{name}"),
                                reason: "required object field is missing".to_owned(),
                            });
                        }
                        None => {}
                    }
                }
                Ok(())
            }
            (_, value) => Err(schema_mismatch(path, self.kind(), value.kind())),
        }
    }

    pub(crate) fn schema_at_path(&self, path: &[String]) -> Option<&Self> {
        path.iter().try_fold(self, |schema, part| match schema {
            Self::Object { fields } => fields.get(part).map(|field| &field.schema),
            _ => None,
        })
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool => "bool",
            Self::Integer => "integer",
            Self::String => "string",
            Self::Enum { .. } => "enum",
            Self::List { .. } => "list",
            Self::Object { .. } => "object",
        }
    }
}

fn schema_mismatch(path: &str, expected: &str, actual: &str) -> WorkflowError {
    WorkflowError::Schema {
        path: path.to_owned(),
        reason: format!("expected {expected}, got {actual}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputSchema {
    Bool {
        default: Option<bool>,
    },
    Integer {
        default: Option<i64>,
    },
    String {
        default: Option<String>,
    },
    Enum {
        members: BTreeSet<WorkflowValue>,
        default: Option<WorkflowValue>,
    },
}

impl InputSchema {
    pub(crate) fn default_value(&self) -> Option<WorkflowValue> {
        match self {
            Self::Bool { default } => default.map(WorkflowValue::Bool),
            Self::Integer { default } => default.map(WorkflowValue::Integer),
            Self::String { default } => default.clone().map(WorkflowValue::String),
            Self::Enum { default, .. } => default.clone(),
        }
    }

    pub(crate) fn validate(&self, value: &WorkflowValue) -> bool {
        match (self, value) {
            (Self::Bool { .. }, WorkflowValue::Bool(_))
            | (Self::Integer { .. }, WorkflowValue::Integer(_))
            | (Self::String { .. }, WorkflowValue::String(_)) => true,
            (Self::Enum { members, .. }, value) => members.contains(value),
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
