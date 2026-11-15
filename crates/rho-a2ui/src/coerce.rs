//! The type coercions of the A2UI core blueprint, applied wherever a value is
//! read as a `DynamicString`, `DynamicNumber` or `DynamicBoolean`.

use serde_json::Value;

use crate::template::parse_number;

/// The largest integer below which every integer has an exact double:
/// 2^53 - 1, the range RFC 8259 names as portable.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// A computed number as a JSON value.
///
/// An integral number within ±(2^53 - 1) becomes an integer, so `1e5` and
/// `100000` are the same value and `-0` is `0`. Others stay doubles, and a
/// non-finite number, which JSON cannot hold, is null.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "the cast only runs on integral values within ±2^53, which i64 holds exactly"
)]
pub(crate) fn number_value(number: f64) -> Value {
    if number.fract() == 0.0 && number.abs() <= MAX_SAFE_INTEGER {
        return Value::from(number as i64);
    }
    serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number)
}

/// A value as a `DynamicString`: null is empty, numbers and booleans are
/// written as JavaScript writes them, and lists and objects as JSON.
pub(crate) fn to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match number.as_f64() {
            Some(float) if number.is_f64() => number_string(float),
            _ => number.to_string(),
        },
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

/// A value read as a number, or `None` when it is not one: numbers as they
/// are, booleans as 1 and 0, and strings when they hold one number literal
/// after trimming, a blank string reading as 0.
pub(crate) fn to_number_checked(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::Bool(flag) => Some(f64::from(u8::from(*flag))),
        Value::String(text) if text.trim().is_empty() => Some(0.0),
        Value::String(text) => parse_number(text.trim()).filter(|n| n.is_finite()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// A value as a `DynamicNumber`: what [`to_number_checked`] reads, else 0.
pub(crate) fn to_number(value: &Value) -> f64 {
    to_number_checked(value).unwrap_or(0.0)
}

/// A value as a `DynamicBoolean`: a `ValidationResult` by its validity,
/// `"true"` and `"false"` in any case, a number by being non-zero, and
/// anything else as false.
pub(crate) fn to_bool(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.eq_ignore_ascii_case("true"),
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::Object(_) => validity(value).unwrap_or(false),
        Value::Null | Value::Array(_) => false,
    }
}

/// The `valid` member of a `ValidationResult`, or `None` for any other value.
pub(crate) fn validity(value: &Value) -> Option<bool> {
    value.get("valid")?.as_bool()
}

/// Writes a finite number as JavaScript's `Number.prototype.toString` does:
/// the shortest digits that round-trip, positional from 1e-6 up to 1e21,
/// and exponential outside, as in `1e+21` and `1.5e-7`.
pub(crate) fn number_string(number: f64) -> String {
    if number == 0.0 || !number.is_finite() {
        return if number.is_nan() {
            "NaN".to_owned()
        } else if number.is_infinite() {
            format!("{}Infinity", if number < 0.0 { "-" } else { "" })
        } else {
            "0".to_owned()
        };
    }
    let sign = if number < 0.0 { "-" } else { "" };
    let scientific = format!("{:e}", number.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` always writes an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().expect("`{:e}` writes an integer exponent");
    // JavaScript's n: the value is 0.digits × 10^n.
    let n = exponent + 1;
    let k = i32::try_from(digits.len()).expect("a double has at most 17 significant digits");
    let body = if k <= n && n <= 21 {
        format!(
            "{digits}{}",
            "0".repeat(usize::try_from(n - k).unwrap_or(0))
        )
    } else if 0 < n && n <= 21 {
        let (whole, fraction) = digits.split_at(usize::try_from(n).unwrap_or(0));
        format!("{whole}.{fraction}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let (first, rest) = digits.split_at(1);
        let point = if rest.is_empty() { "" } else { "." };
        let exp_sign = if n > 0 { "+" } else { "-" };
        format!("{first}{point}{rest}e{exp_sign}{}", (n - 1).abs())
    };
    format!("{sign}{body}")
}

#[cfg(test)]
#[path = "coerce_tests.rs"]
mod tests;
