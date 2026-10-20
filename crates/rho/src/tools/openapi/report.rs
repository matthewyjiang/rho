//! What loading OpenAPI sources tells the model and the user.

/// Loaded sources for the prompt, and problems for the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenApiReport {
    pub(crate) sources: Vec<LoadedSource>,
    /// Sources that did not load, one line each.
    pub(crate) errors: Vec<String>,
    /// Sources that loaded from a stale cache or were ignored.
    pub(crate) warnings: Vec<String>,
}

/// Tags listed per source in the prompt line; the rest are counted.
const PROMPT_TAGS: usize = 6;

/// Spec text is untrusted, so a tag shown in the prompt is one short line.
const PROMPT_TAG_CHARS: usize = 40;

impl OpenApiReport {
    /// One prompt line per loaded source, beside the MCP server list. The
    /// operations stay out of context until `tool_search` loads them.
    pub(crate) fn prompt_context(&self) -> String {
        let sources: Vec<&LoadedSource> = self
            .sources
            .iter()
            .filter(|source| source.read_operations > 0)
            .collect();
        if sources.is_empty() {
            return String::new();
        }
        let mut text = String::from(
            "\n\n# OpenAPI sources\n\nRead-only REST operations, found and called like MCP tools through `tool_search` or `codemode`:\n",
        );
        for source in sources {
            text.push_str(&format!(
                "- {}: {} read operations",
                source.name, source.read_operations
            ));
            if !source.tags.is_empty() {
                let shown: Vec<String> = source
                    .tags
                    .iter()
                    .take(PROMPT_TAGS)
                    .map(|tag| prompt_tag(tag))
                    .collect();
                let more = if source.tags.len() > PROMPT_TAGS {
                    ", …"
                } else {
                    ""
                };
                text.push_str(&format!(
                    ", {} tags ({}{more})",
                    source.tags.len(),
                    shown.join(", ")
                ));
            }
            text.push('\n');
        }
        text
    }
}

fn prompt_tag(tag: &str) -> String {
    let tag: String = tag
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    match tag.char_indices().nth(PROMPT_TAG_CHARS) {
        Some((cut, _)) => format!("{}…", &tag[..cut]),
        None => tag,
    }
}

/// One source whose tools are registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadedSource {
    pub(crate) name: String,
    pub(crate) read_operations: usize,
    /// Tags of the registered operations, most used first.
    pub(crate) tags: Vec<String>,
}
