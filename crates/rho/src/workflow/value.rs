use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};

use super::{WorkflowError, WorkflowResult};

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum WorkflowValue {
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
    List(Vec<WorkflowValue>),
    Object(BTreeMap<String, WorkflowValue>),
}

impl WorkflowValue {
    pub(crate) fn from_json(value: serde_json::Value) -> WorkflowResult<Self> {
        Self::from_json_at(value, "$")
    }

    fn from_json_at(value: serde_json::Value, path: &str) -> WorkflowResult<Self> {
        match value {
            serde_json::Value::Null => Ok(Self::Null),
            serde_json::Value::Bool(value) => Ok(Self::Bool(value)),
            serde_json::Value::Number(value) => {
                value
                    .as_i64()
                    .map(Self::Integer)
                    .ok_or_else(|| WorkflowError::UnsupportedValue {
                        path: path.to_owned(),
                        kind: format!("number '{value}' does not fit i64 or is a float"),
                    })
            }
            serde_json::Value::String(value) => Ok(Self::String(value)),
            serde_json::Value::Array(values) => values
                .into_iter()
                .enumerate()
                .map(|(index, value)| Self::from_json_at(value, &format!("{path}[{index}]")))
                .collect::<WorkflowResult<_>>()
                .map(Self::List),
            serde_json::Value::Object(values) => values
                .into_iter()
                .map(|(key, value)| {
                    let value = Self::from_json_at(value, &format!("{path}.{key}"))?;
                    Ok((key, value))
                })
                .collect::<WorkflowResult<_>>()
                .map(Self::Object),
        }
    }

    pub(crate) fn scalar(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Bool(_) | Self::Integer(_) | Self::String(_)
        )
    }

    pub(crate) fn at_path<'a>(&'a self, path: &[String]) -> Option<&'a Self> {
        path.iter().try_fold(self, |value, part| match value {
            Self::Object(values) => values.get(part),
            _ => None,
        })
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Integer(_) => "integer",
            Self::String(_) => "string",
            Self::List(_) => "list",
            Self::Object(_) => "object",
        }
    }
}

impl fmt::Display for WorkflowValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => formatter.write_str(value),
            _ => write!(
                formatter,
                "{}",
                serde_json::to_string(self).map_err(|_| fmt::Error)?
            ),
        }
    }
}
