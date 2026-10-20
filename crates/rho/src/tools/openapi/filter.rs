//! `allow` and `deny` globs on an operation's id or `METHOD /path`.
//!
//! `*` matches any run of characters, `/` included, and `?` matches one
//! character. Matching is case-sensitive; methods are written uppercase, as in
//! `GET /repos/*`. `deny` is checked first, and an empty `allow` admits every
//! operation that is not denied.

use super::operation::Operation;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OperationFilter {
    allow: Vec<String>,
    deny: Vec<String>,
}

impl OperationFilter {
    pub(crate) fn new(allow: &[String], deny: &[String]) -> Self {
        Self {
            allow: allow.to_vec(),
            deny: deny.to_vec(),
        }
    }

    pub(crate) fn admits(&self, operation: &Operation) -> bool {
        let route = format!(
            "{} {}",
            operation.method.as_str().to_ascii_uppercase(),
            operation.path
        );
        let matches = |pattern: &String| {
            glob_matches(pattern, &route)
                || operation
                    .operation_id
                    .as_deref()
                    .is_some_and(|operation_id| glob_matches(pattern, operation_id))
        };
        !self.deny.iter().any(matches) && (self.allow.is_empty() || self.allow.iter().any(matches))
    }
}

/// Iterative wildcard match with single-star backtracking.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    // The last `*` seen and the text position it currently absorbs up to.
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&character) if character == '?' || character == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|character| *character == '*')
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
