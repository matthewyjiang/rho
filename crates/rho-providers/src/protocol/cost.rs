use serde_json::Value;

pub(super) fn parse_usd_micros(value: &Value) -> Option<u64> {
    let dollars = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(raw) => parse_usd_string(raw)?,
        Value::Object(_) => object_usd_dollars(value)?,
        _ => return None,
    };
    dollars_to_micros(dollars)
}

fn dollars_to_micros(dollars: f64) -> Option<u64> {
    let micros = dollars * 1_000_000.0;
    (dollars >= 0.0 && micros.is_finite() && micros < u64::MAX as f64)
        .then(|| micros.round() as u64)
}

/// Structured `usage.cost` from composer-api and similar OpenAI-compatible hosts.
///
/// Totals use `*_usd` keys. Catalog-style `{ input, output }` rates are ignored
/// so a models-list shape cannot be mistaken for a dollar amount.
fn object_usd_dollars(value: &Value) -> Option<f64> {
    for key in ["total_usd", "cost_usd"] {
        if let Some(dollars) = value.get(key).and_then(non_negative_usd_dollars) {
            return Some(dollars);
        }
    }
    let input = value.get("input_usd").and_then(non_negative_usd_dollars);
    let output = value.get("output_usd").and_then(non_negative_usd_dollars);
    match (input, output) {
        (Some(input), Some(output)) => Some(input + output),
        (Some(input), None) => Some(input),
        (None, Some(output)) => Some(output),
        (None, None) => None,
    }
}

fn non_negative_usd_dollars(value: &Value) -> Option<f64> {
    let dollars = scalar_usd_dollars(value)?;
    (dollars.is_finite() && dollars >= 0.0).then_some(dollars)
}

fn scalar_usd_dollars(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(raw) => parse_usd_string(raw),
        _ => None,
    }
}

fn parse_usd_string(raw: &str) -> Option<f64> {
    let raw = raw.trim();
    let amount = raw.strip_prefix('$').unwrap_or(raw);
    let (integer, fraction) = amount.split_once('.').unwrap_or((amount, "0"));
    if integer.is_empty()
        || fraction.is_empty()
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    if integer.contains(',') {
        let mut groups = integer.split(',');
        let first = groups.next()?;
        if first.is_empty()
            || first.len() > 3
            || !first.bytes().all(|byte| byte.is_ascii_digit())
            || groups
                .any(|group| group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
    } else if !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    amount.replace(',', "").parse().ok()
}

#[cfg(test)]
#[path = "cost_tests.rs"]
mod cost_tests;
