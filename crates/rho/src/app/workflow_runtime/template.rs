//! Bounded template expansion at the invocation preparation boundary.
use std::collections::BTreeMap;

use crate::workflow::{FrozenRuntimeLimits, NodeId, Template, TemplatePart, WorkflowValue};

use super::RuntimeError;

pub(super) fn render_template(
    template: &Template,
    outputs: &BTreeMap<NodeId, WorkflowValue>,
    limits: &FrozenRuntimeLimits,
) -> Result<String, RuntimeError> {
    let mut rendered = String::new();
    for part in &template.0 {
        match part {
            TemplatePart::Literal { value } => {
                append_bounded(&mut rendered, value, limits.rendered_template_bytes)?
            }
            TemplatePart::Output { reference } => {
                let value = outputs
                    .get(&reference.node)
                    .and_then(|value| value.at_path(&reference.path.0))
                    .ok_or_else(|| {
                        RuntimeError::Data(format!(
                            "required output '{}.{}' is unavailable",
                            reference.node,
                            reference.path.0.join(".")
                        ))
                    })?;
                append_bounded(
                    &mut rendered,
                    &value.to_string(),
                    limits.rendered_template_bytes,
                )?;
            }
        }
    }
    Ok(rendered)
}

fn append_bounded(output: &mut String, value: &str, limit: u64) -> Result<(), RuntimeError> {
    let requested = output
        .len()
        .checked_add(value.len())
        .map(|value| value as u64)
        .unwrap_or(u64::MAX);
    check_runtime_limit("rendered template bytes", limit, requested)?;
    output
        .try_reserve(value.len())
        .map_err(|error| RuntimeError::Executor(format!("template allocation failed: {error}")))?;
    output.push_str(value);
    Ok(())
}

pub(super) fn check_runtime_limit(
    budget: &'static str,
    limit: u64,
    actual: u64,
) -> Result<(), RuntimeError> {
    if actual > limit {
        return Err(crate::workflow::WorkflowError::BudgetExceeded {
            budget,
            limit,
            actual,
        }
        .into());
    }
    Ok(())
}
