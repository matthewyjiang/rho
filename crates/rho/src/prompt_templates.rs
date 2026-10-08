use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub type PromptTemplates = BTreeMap<String, String>;

pub(crate) fn discover_with_home(cwd: &Path, home: Option<&Path>) -> PromptTemplates {
    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".rho").join("prompts"));
    }
    roots.extend(
        crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .rev()
            .map(|path| path.join(".rho").join("prompts")),
    );

    let mut templates = PromptTemplates::new();
    for root in roots {
        for path in template_paths(&root) {
            let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(template) = std::fs::read_to_string(&path) else {
                continue;
            };
            if validate_entry(name, &template).is_ok() {
                insert_override(
                    &mut templates,
                    name.to_string(),
                    template.trim().to_string(),
                );
            }
        }
    }
    templates
}

fn template_paths(root: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("md" | "txt")
                )
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

pub fn validate(templates: &PromptTemplates) -> anyhow::Result<()> {
    let mut names = BTreeSet::new();
    for (name, template) in templates {
        validate_entry(name, template)?;
        if !names.insert(name.to_ascii_lowercase()) {
            anyhow::bail!(
                "prompt template name '/{name}' conflicts with another template name (names are case-insensitive)"
            );
        }
    }
    Ok(())
}

pub fn merge(templates: &mut PromptTemplates, overrides: PromptTemplates) {
    for (name, template) in overrides {
        insert_override(templates, name, template);
    }
}

fn insert_override(templates: &mut PromptTemplates, name: String, template: String) {
    if let Some(existing) = templates
        .keys()
        .find(|existing| existing.eq_ignore_ascii_case(&name))
        .cloned()
    {
        templates.remove(&existing);
    }
    templates.insert(name, template);
}

fn validate_entry(name: &str, template: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        anyhow::bail!(
            "invalid prompt template name '{name}': use only letters, numbers, '-' and '_'"
        );
    }
    if crate::commands::COMMANDS
        .iter()
        .any(|command| command.name.eq_ignore_ascii_case(name))
    {
        anyhow::bail!("prompt template '/{name}' conflicts with a built-in command");
    }
    if template.trim().is_empty() {
        anyhow::bail!("prompt template '/{name}' cannot be empty");
    }
    Ok(())
}

pub fn description(template: &str) -> String {
    template.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn matches_search(name: &str, search: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with(search) || format!("prompt:{name}").starts_with(search)
}

pub fn find<'a>(templates: &'a PromptTemplates, name: &str) -> Option<&'a str> {
    templates
        .iter()
        .find(|(template_name, _)| template_name.eq_ignore_ascii_case(name))
        .map(|(_, template)| template.as_str())
}

/// The template name in a namespaced command, with a case-insensitive prefix.
pub(crate) fn command_template_name(command: &str) -> Option<&str> {
    const PREFIX: &str = "prompt:";
    command
        .get(..PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
        .map(|_| &command[PREFIX.len()..])
}

/// Expands a template with the text typed after its command.
///
/// `$ARGUMENTS` takes all of that text and `$1`..`$n` take its
/// whitespace-separated arguments; double quotes group words with spaces.
/// A missing positional argument expands to nothing. A template without
/// placeholders gets the text appended instead.
pub fn expand(template: &str, trailing_text: &str) -> String {
    let trailing_text = trailing_text.trim();
    let arguments = split_arguments(trailing_text);
    let mut expanded = String::with_capacity(template.len() + trailing_text.len());
    let mut rest = template;
    let mut substituted = false;
    while let Some(index) = rest.find('$') {
        expanded.push_str(&rest[..index]);
        let after = &rest[index + 1..];
        match placeholder(after) {
            Some((Placeholder::All, len)) => {
                substituted = true;
                expanded.push_str(trailing_text);
                rest = &after[len..];
            }
            Some((Placeholder::Position(position), len)) => {
                substituted = true;
                if let Some(argument) = arguments.get(position - 1) {
                    expanded.push_str(argument);
                }
                rest = &after[len..];
            }
            None => {
                expanded.push('$');
                rest = after;
            }
        }
    }
    expanded.push_str(rest);
    if !substituted && !trailing_text.is_empty() {
        expanded.push(' ');
        expanded.push_str(trailing_text);
    }
    expanded
}

/// Whether the template places its arguments itself instead of having them
/// appended.
pub fn takes_arguments(template: &str) -> bool {
    placeholders(template).next().is_some()
}

/// Palette usage hint naming the arguments the template places.
///
/// Positions are listed up to the highest one used, since a gap still
/// consumes an argument. Past three they collapse to `[$1] … [$n]` so a
/// stray `$100` cannot flood the row. All are optional because a missing
/// argument expands to nothing.
pub fn usage(command_name: &str, template: &str) -> String {
    const LISTED_POSITIONS: usize = 3;
    let mut highest = 0;
    let mut all = false;
    for placeholder in placeholders(template) {
        match placeholder {
            Placeholder::All => all = true,
            Placeholder::Position(position) => highest = highest.max(position),
        }
    }
    let mut usage = format!("/{command_name}");
    if highest == 0 && !all {
        usage.push_str(" [text]");
        return usage;
    }
    if highest <= LISTED_POSITIONS {
        for position in 1..=highest {
            usage.push_str(&format!(" [${position}]"));
        }
    } else {
        usage.push_str(&format!(" [$1] … [${highest}]"));
    }
    if all {
        usage.push_str(" [arguments]");
    }
    usage
}

fn placeholders(template: &str) -> impl Iterator<Item = Placeholder> + '_ {
    template
        .match_indices('$')
        .filter_map(|(index, _)| placeholder(&template[index + 1..]))
        .map(|(placeholder, _)| placeholder)
}

enum Placeholder {
    /// `$ARGUMENTS`: all trailing text.
    All,
    /// `$1`..`$n`: one argument, counting from 1.
    Position(usize),
}

/// The placeholder right after a `$`, with its byte length (excluding `$`).
fn placeholder(after_dollar: &str) -> Option<(Placeholder, usize)> {
    const ALL: &str = "ARGUMENTS";
    if after_dollar.starts_with(ALL) {
        return Some((Placeholder::All, ALL.len()));
    }
    let digits = after_dollar.bytes().take_while(u8::is_ascii_digit).count();
    match after_dollar[..digits].parse::<usize>() {
        Ok(position) if position > 0 => Some((Placeholder::Position(position), digits)),
        _ => None,
    }
}

/// Splits arguments on whitespace. Double quotes group a value with spaces and
/// are removed; single quotes stay literal so apostrophes in prose survive. An
/// unclosed quote runs to the end of the text.
fn split_arguments(text: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut in_argument = false;
    let mut quoted = false;
    for ch in text.chars() {
        if ch == '"' {
            quoted = !quoted;
            in_argument = true;
        } else if ch.is_whitespace() && !quoted {
            if in_argument {
                arguments.push(std::mem::take(&mut current));
                in_argument = false;
            }
        } else {
            current.push(ch);
            in_argument = true;
        }
    }
    if in_argument {
        arguments.push(current);
    }
    arguments
}

#[cfg(test)]
#[path = "prompt_templates_tests.rs"]
mod tests;
