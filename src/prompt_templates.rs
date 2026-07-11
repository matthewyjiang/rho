use std::{collections::BTreeMap, path::Path};

pub type PromptTemplates = BTreeMap<String, String>;

pub fn discover(cwd: &Path) -> PromptTemplates {
    discover_with_home(cwd, crate::paths::home_dir().as_deref())
}

fn discover_with_home(cwd: &Path, home: Option<&Path>) -> PromptTemplates {
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
                templates.insert(name.to_string(), template.trim().to_string());
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
    for (name, template) in templates {
        validate_entry(name, template)?;
    }
    Ok(())
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

pub fn find<'a>(templates: &'a PromptTemplates, name: &str) -> Option<&'a str> {
    templates
        .iter()
        .find(|(template_name, _)| template_name.eq_ignore_ascii_case(name))
        .map(|(_, template)| template.as_str())
}

pub fn expand(template: &str, trailing_text: &str) -> String {
    let trailing_text = trailing_text.trim();
    if trailing_text.is_empty() {
        template.to_string()
    } else {
        format!("{template} {trailing_text}")
    }
}

#[cfg(test)]
#[path = "prompt_templates_tests.rs"]
mod tests;
