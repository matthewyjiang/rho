//! Shared budgeted assembly for background-completion notifications.

/// Joins status sections until `budget` is exhausted, then an omission line
/// if that still fits.
pub(crate) fn join_budgeted_sections<I, S>(
    sections: I,
    separator: &str,
    budget: usize,
    omit: impl Fn(usize) -> String,
) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let sections: Vec<S> = sections.into_iter().collect();
    let mut body = String::new();
    for (index, section) in sections.iter().enumerate() {
        let section = section.as_ref();
        let separator = if index == 0 { "" } else { separator };
        if body.len() + separator.len() + section.len() > budget {
            let omission = omit(sections.len() - index);
            let omission = if body.is_empty() {
                omission
            } else {
                format!("{separator}{omission}")
            };
            if body.len() + omission.len() <= budget {
                body.push_str(&omission);
            }
            break;
        }
        body.push_str(separator);
        body.push_str(section);
    }
    body
}
