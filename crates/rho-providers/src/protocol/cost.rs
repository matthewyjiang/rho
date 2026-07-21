use serde_json::Value;

pub(super) fn parse_usd_micros(value: &Value) -> Option<u64> {
    let dollars = match value {
        Value::Number(number) => number.as_f64()?,
        Value::String(raw) => parse_usd_string(raw)?,
        _ => return None,
    };
    let micros = dollars * 1_000_000.0;
    (dollars >= 0.0 && micros.is_finite() && micros < u64::MAX as f64)
        .then(|| micros.round() as u64)
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
