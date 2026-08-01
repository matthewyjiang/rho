use sha2::{Digest as _, Sha256};

use super::{Digest, FrozenWorkflow, WorkflowError, WorkflowResult};

const DOMAIN: &[u8] = b"rho-workflow-graph-v1\0";

pub(crate) fn canonical_bytes(workflow: &FrozenWorkflow) -> WorkflowResult<Vec<u8>> {
    let mut value = workflow.clone();
    value.graph_digest = Digest(String::new());
    let json = serde_json::to_value(value)?;
    let mut output = Vec::new();
    output.extend_from_slice(DOMAIN);
    encode_value(&json, &mut output)?;
    Ok(output)
}

pub(crate) fn graph_digest(workflow: &FrozenWorkflow) -> WorkflowResult<Digest> {
    let digest = Sha256::digest(canonical_bytes(workflow)?);
    Ok(Digest(format!("sha256:{digest:x}")))
}

// Tags are part of graph format v1: null, false, true, signed integer,
// unsigned integer, string, vector, object. Maps arrive sorted because all
// workflow maps use BTreeMap; sorting again guards future serde map changes.
fn encode_value(value: &serde_json::Value, output: &mut Vec<u8>) -> WorkflowResult<()> {
    match value {
        serde_json::Value::Null => output.push(0),
        serde_json::Value::Bool(false) => output.push(1),
        serde_json::Value::Bool(true) => output.push(2),
        serde_json::Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                output.push(3);
                output.extend_from_slice(&value.to_be_bytes());
            } else if let Some(value) = number.as_u64() {
                output.push(4);
                output.extend_from_slice(&value.to_be_bytes());
            } else {
                return Err(WorkflowError::UnsupportedValue {
                    path: "$canonical".to_owned(),
                    kind: format!("floating-point number {number}"),
                });
            }
        }
        serde_json::Value::String(value) => {
            output.push(5);
            encode_bytes(value.as_bytes(), output)?;
        }
        serde_json::Value::Array(values) => {
            output.push(6);
            encode_len(values.len(), output)?;
            for value in values {
                encode_value(value, output)?;
            }
        }
        serde_json::Value::Object(values) => {
            output.push(7);
            encode_len(values.len(), output)?;
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
            for (key, value) in entries {
                encode_bytes(key.as_bytes(), output)?;
                encode_value(value, output)?;
            }
        }
    }
    Ok(())
}

fn encode_bytes(bytes: &[u8], output: &mut Vec<u8>) -> WorkflowResult<()> {
    encode_len(bytes.len(), output)?;
    output.extend_from_slice(bytes);
    Ok(())
}

fn encode_len(len: usize, output: &mut Vec<u8>) -> WorkflowResult<()> {
    let len = u64::try_from(len).map_err(|_| WorkflowError::BudgetExceeded {
        budget: "canonical collection bytes",
        limit: u64::MAX,
        actual: u64::MAX,
    })?;
    output.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod tests;
