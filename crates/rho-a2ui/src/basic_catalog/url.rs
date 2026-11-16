//! `openUrl`.

use serde_json::Value;

use crate::data_context::DataContext;
use crate::functions::{Args, Effect, FunctionError};

/// The schemes `openUrl` opens. Any other, such as `javascript:` or `file:`,
/// could run code or read local files.
const ALLOWED_SCHEMES: [&str; 4] = ["http", "https", "mailto", "tel"];

/// `openUrl(url)`: asks the host to open an `http`, `https`, `mailto` or
/// `tel` URL, with [`Effect::OpenUrl`]. A blank URL does nothing.
pub(crate) fn open_url(args: &Args, context: &mut DataContext<'_>) -> Result<Value, FunctionError> {
    let url = args.string("url");
    let url = url.trim();
    if url.is_empty() {
        return Ok(Value::Null);
    }
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme)
        .filter(|scheme| {
            scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        })
        .ok_or_else(|| FunctionError::RelativeUrl {
            url: url.to_owned(),
        })?;
    if !ALLOWED_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(FunctionError::UnsupportedUrlScheme {
            scheme: scheme.to_owned(),
        });
    }
    context.push_effect(Effect::OpenUrl(url.to_owned()));
    Ok(Value::Null)
}

#[cfg(test)]
#[path = "url_tests.rs"]
mod tests;
