/// Whether `name` is a UAX #31 identifier, as A2UI v1.0 requires of component,
/// function, property and argument names and of component ids.
///
/// The first character is `XID_Start` or `_`, every later one `XID_Continue`.
/// That rules out a leading digit, whitespace and the `Pattern_Syntax`
/// characters such as `-`, `#` and `@`. The empty string is not an identifier.
#[must_use]
pub fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || unicode_ident::is_xid_start(first))
        && chars.all(unicode_ident::is_xid_continue)
}
