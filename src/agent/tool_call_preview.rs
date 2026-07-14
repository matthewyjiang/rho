use crate::{
    agent::questionnaire,
    tool::{ToolContext, ToolRegistry},
};

pub(super) fn display_lines(
    name: Option<&str>,
    arguments: &str,
    tools: &ToolRegistry,
    ctx: &ToolContext,
) -> Vec<String> {
    let Some(name) = name.filter(|name| !name.is_empty()) else {
        return Vec::new();
    };
    let arguments = serde_json::from_str(arguments)
        .ok()
        .or_else(|| parse_partial_json(arguments));
    let Some(arguments) = arguments else {
        return vec![name.into()];
    };
    if name == questionnaire::TOOL_NAME {
        return questionnaire::parse_request(arguments).map_or_else(
            |_| vec![name.into()],
            |request| questionnaire::start_display_lines(&request),
        );
    }
    tools.get(name).map_or_else(
        || vec![name.into()],
        |tool| tool.display_start_lines(&arguments, ctx),
    )
}

fn parse_partial_json(input: &str) -> Option<serde_json::Value> {
    let mut commas = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut unicode_digits_remaining = 0;

    for (index, character) in input.char_indices() {
        if in_string {
            if unicode_digits_remaining > 0 {
                if character.is_ascii_hexdigit() {
                    unicode_digits_remaining -= 1;
                } else {
                    unicode_digits_remaining = 0;
                }
                continue;
            }
            if escaped {
                escaped = false;
                if character == 'u' {
                    unicode_digits_remaining = 4;
                }
                continue;
            }
            match character {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
        } else {
            match character {
                '"' => in_string = true,
                ',' => commas.push(index),
                _ => {}
            }
        }
    }

    complete_partial_json(input).or_else(|| {
        commas
            .into_iter()
            .rev()
            .find_map(|comma| complete_partial_json(&input[..comma]))
    })
}

fn complete_partial_json(input: &str) -> Option<serde_json::Value> {
    let mut suffix = String::new();
    let mut containers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut unicode_digits_remaining = 0;

    for character in input.chars() {
        if in_string {
            if unicode_digits_remaining > 0 {
                if character.is_ascii_hexdigit() {
                    unicode_digits_remaining -= 1;
                } else {
                    unicode_digits_remaining = 0;
                }
                continue;
            }
            if escaped {
                escaped = false;
                if character == 'u' {
                    unicode_digits_remaining = 4;
                }
                continue;
            }
            match character {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '{' => containers.push('}'),
            '[' => containers.push(']'),
            '}' | ']' => {
                containers.pop();
            }
            _ => {}
        }
    }

    if in_string {
        if unicode_digits_remaining > 0 {
            suffix.extend(std::iter::repeat_n('0', unicode_digits_remaining));
        } else if escaped {
            suffix.push('\\');
        }
        suffix.push('"');
    }
    suffix.extend(containers.into_iter().rev());

    let mut completed = String::with_capacity(input.len() + suffix.len());
    completed.push_str(input);
    completed.push_str(&suffix);
    serde_json::from_str(&completed).ok()
}

#[cfg(test)]
#[path = "tool_call_preview_tests.rs"]
mod tests;
