use std::collections::BTreeMap;

pub type PromptTemplates = BTreeMap<String, String>;

pub fn validate(templates: &PromptTemplates) -> anyhow::Result<()> {
    for (name, template) in templates {
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
