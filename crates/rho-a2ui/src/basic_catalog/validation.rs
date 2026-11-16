//! The validators. Each returns a `ValidationResult`: `{"valid": true}`, or
//! `{"valid": false, "message": ...}` with a message a renderer may show.

use serde_json::{json, Value};

use crate::coerce::{number_string, to_number_checked};
use crate::functions::{Args, FunctionError};
use crate::regex_dialect::Pattern;

fn valid() -> Value {
    json!({"valid": true})
}

fn invalid(message: &str) -> Value {
    json!({"valid": false, "message": message})
}

/// `required(value)`: anything but null, an empty string or an empty list.
pub(crate) fn required(args: &Args) -> Value {
    let present = match args.get("value") {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    };
    if present {
        valid()
    } else {
        invalid("This field is required.")
    }
}

/// `regex(value, pattern)`: whether `pattern`, in the A2UI regex dialect,
/// matches anywhere in `value`.
pub(crate) fn regex(args: &Args) -> Result<Value, FunctionError> {
    let pattern = Pattern::new(&args.string("pattern"))?;
    Ok(if pattern.is_match(&args.string("value"))? {
        valid()
    } else {
        invalid("Value does not match required pattern.")
    })
}

/// `length(value, min, max)`: the code points of a string, or the entries of
/// a list, are within the bounds given. Any other value has length 0.
pub(crate) fn length(args: &Args) -> Value {
    let length = match args.get("value") {
        Some(Value::String(text)) => text.chars().count(),
        Some(Value::Array(items)) => items.len(),
        _ => 0,
    };
    let length = u32::try_from(length).map_or(f64::INFINITY, f64::from);
    check_bounds(args, length, "length")
}

/// `numeric(value, min, max)`: the value reads as a number within the bounds
/// given.
pub(crate) fn numeric(args: &Args) -> Value {
    let Some(number) = args.get("value").and_then(to_number_checked) else {
        return invalid("Value must be a valid number.");
    };
    check_bounds(args, number, "value")
}

/// Checks `measure` against the `min` and `max` arguments that read as numbers.
fn check_bounds(args: &Args, measure: f64, what: &str) -> Value {
    let bound = |name| args.get(name).and_then(to_number_checked);
    if let Some(min) = bound("min").filter(|&min| measure < min) {
        return invalid(&format!("Minimum {what} is {}.", number_string(min)));
    }
    if let Some(max) = bound("max").filter(|&max| measure > max) {
        return invalid(&format!("Maximum {what} is {}.", number_string(max)));
    }
    valid()
}

/// `email(value)`: a string of the form `local@domain.tld`, with ASCII
/// letters, digits and `._%+-` before the `@`, letters, digits, `.` and `-`
/// in the domain, and a top-level domain of at least two letters.
pub(crate) fn email(args: &Args) -> Value {
    if matches!(args.get("value"), Some(Value::String(text)) if is_email(text)) {
        valid()
    } else {
        json!({
            "valid": false,
            "code": "INVALID_EMAIL",
            "message": "Please enter a valid email address (e.g. user@example.com).",
        })
    }
}

fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    let all = |s: &str, ok: fn(char) -> bool| !s.is_empty() && s.chars().all(ok);
    all(local, |c| c.is_ascii_alphanumeric() || "._%+-".contains(c))
        && all(host, |c| c.is_ascii_alphanumeric() || ".-".contains(c))
        && tld.len() >= 2
        && all(tld, |c| c.is_ascii_alphabetic())
}
