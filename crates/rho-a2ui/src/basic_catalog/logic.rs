//! `and`, `or` and `not`, over `DynamicBoolean` operands.

use serde_json::Value;

use crate::coerce::to_bool;
use crate::functions::{Args, FunctionError};

/// The `values` argument as booleans. It must list at least two.
fn operands(args: &Args) -> Result<impl Iterator<Item = bool> + '_, FunctionError> {
    match args.get("values") {
        Some(Value::Array(values)) if values.len() >= 2 => Ok(values.iter().map(to_bool)),
        Some(Value::Array(values)) => Err(FunctionError::TooFewValues { got: values.len() }),
        _ => Err(FunctionError::ValuesNotAList),
    }
}

/// `and(values)`: every value is true.
pub(crate) fn and(args: &Args) -> Result<Value, FunctionError> {
    Ok(Value::Bool(operands(args)?.all(|value| value)))
}

/// `or(values)`: some value is true.
pub(crate) fn or(args: &Args) -> Result<Value, FunctionError> {
    Ok(Value::Bool(operands(args)?.any(|value| value)))
}

/// `not(value)`: the value is false.
pub(crate) fn not(args: &Args) -> Value {
    Value::Bool(!args.boolean("value"))
}
