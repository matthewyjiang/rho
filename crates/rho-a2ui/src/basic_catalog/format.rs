//! `formatString`, `formatNumber`, `formatCurrency` and `pluralize`. Numbers
//! are formatted as `Intl.NumberFormat` formats them for `en-US`.

use serde_json::Value;

use crate::coerce::{to_bool, to_number, to_string};
use crate::data_context::DataContext;
use crate::functions::{Args, FunctionError};
use crate::template::parse_template;

/// `formatString(value)`: parses `value` as a template and joins its parts,
/// each evaluated in the caller's context and written as a `DynamicString`.
pub(crate) fn format_string(
    args: &Args,
    context: &mut DataContext<'_>,
) -> Result<Value, FunctionError> {
    let parts = parse_template(&args.string("value")).map_err(Box::new)?;
    let mut text = String::new();
    for part in &parts {
        text.push_str(&to_string(&context.evaluate(part).map_err(Box::new)?));
    }
    Ok(Value::String(text))
}

/// `formatNumber(value, decimals, grouping)`: exactly `decimals` fraction
/// digits when given, else up to three.
pub(crate) fn format_number(args: &Args) -> Value {
    let (min, max) = decimals(args).map_or((0, 3), |d| (d, d));
    let number = args.number("value");
    Value::String(decimal(number, min, max, grouping(args)))
}

/// `formatCurrency(value, currency, decimals, grouping)`: the amount after
/// the currency's `en-US` symbol, or after its code for a currency without
/// one, with `decimals` fraction digits, 2 by default.
pub(crate) fn format_currency(args: &Args) -> Value {
    let digits = decimals(args).unwrap_or(2);
    let number = args.number("value");
    let amount = decimal(number.abs(), digits, digits, grouping(args));
    let code = args.string("currency").to_uppercase();
    let sign = if number < 0.0 { "-" } else { "" };
    Value::String(match symbol(&code) {
        Some(symbol) => format!("{sign}{symbol}{amount}"),
        None => format!("{sign}{code}\u{a0}{amount}"),
    })
}

/// The symbols `en-US` uses for common currencies, from CLDR.
fn symbol(code: &str) -> Option<&'static str> {
    Some(match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "CNY" => "CN¥",
        "INR" => "₹",
        "CAD" => "CA$",
        "AUD" => "A$",
        _ => return None,
    })
}

/// `pluralize(value, zero, one, two, few, many, other)`: the form for the
/// count. An explicit `zero`, `one` or `two` form wins for exactly 0, 1 or
/// 2; otherwise English's plural rule picks `one` or `other`, and a missing
/// form falls back to `other`. A form given as an empty string is used.
#[expect(clippy::float_cmp, reason = "plural categories name exact counts")]
pub(crate) fn pluralize(args: &Args) -> Value {
    let count = args.number("value");
    let explicit = [("zero", 0.0), ("one", 1.0), ("two", 2.0)]
        .into_iter()
        .find(|&(name, exact)| count == exact && args.get(name).is_some());
    let category = match explicit {
        Some((name, _)) => name,
        None if count == 1.0 => "one",
        None => "other",
    };
    let form = args.get(category).or_else(|| args.get("other"));
    Value::String(form.map(to_string).unwrap_or_default())
}

/// The `decimals` argument as a digit count from 0 to 20.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=20 and truncated first"
)]
fn decimals(args: &Args) -> Option<usize> {
    let digits = to_number(args.get("decimals")?);
    Some(digits.trunc().clamp(0.0, 20.0) as usize)
}

/// Whether to group thousands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    On,
    Off,
}

/// The `grouping` argument, a `DynamicBoolean` that defaults to on.
fn grouping(args: &Args) -> Grouping {
    match args.get("grouping") {
        Some(value) if !value.is_null() && !to_bool(value) => Grouping::Off,
        _ => Grouping::On,
    }
}

/// Writes `number` with `.` before at least `min_fraction` and at most
/// `max_fraction` fraction digits, rounding half away from zero on the
/// double's exact value as `Intl.NumberFormat` does, and with `,` between
/// thousands when grouping.
fn decimal(number: f64, min_fraction: usize, max_fraction: usize, grouping: Grouping) -> String {
    // A double's exact expansion ends within 1074 fraction digits.
    let exact = format!("{:.1100}", number.abs());
    let (whole, fraction) = exact.split_once('.').expect("a precision writes a point");
    let mut digits: Vec<u8> = whole
        .bytes()
        .chain(fraction.bytes().take(max_fraction))
        .collect();
    let mut whole_len = whole.len();
    if fraction
        .as_bytes()
        .get(max_fraction)
        .is_some_and(|&d| d >= b'5')
    {
        let carried = digits.iter_mut().rev().all(|d| {
            let nine = *d == b'9';
            *d = if nine { b'0' } else { *d + 1 };
            nine
        });
        if carried {
            digits.insert(0, b'1');
            whole_len += 1;
        }
    }
    let (whole, fraction) = digits.split_at(whole_len);
    let keep = fraction
        .iter()
        .rposition(|&d| d != b'0')
        .map_or(0, |last| last + 1)
        .max(min_fraction);
    let mut text = String::new();
    if number < 0.0 {
        text.push('-');
    }
    for (i, &digit) in whole.iter().enumerate() {
        if grouping == Grouping::On && i > 0 && (whole.len() - i) % 3 == 0 {
            text.push(',');
        }
        text.push(char::from(digit));
    }
    if keep > 0 {
        text.push('.');
        text.extend(fraction[..keep].iter().map(|&d| char::from(d)));
    }
    text
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
