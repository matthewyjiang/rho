pub(super) fn contains_unsafe_content(source: &str) -> bool {
    if source.chars().any(|character| {
        character == '\x1b' || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    }) {
        return true;
    }
    let lower = source.to_ascii_lowercase();
    lower.contains("javascript:")
        || lower.contains("<script")
        || lower.contains("</script")
        || lower.contains("<iframe")
        || lower.contains("<a ")
        || lower.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("click ")
                || trimmed.starts_with("href ")
                || trimmed.starts_with("link ")
                || trimmed.starts_with("callback ")
        })
}
