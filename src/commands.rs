use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandId {
    Login,
    Logout,
    Model,
    Resume,
    Config,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInvocation {
    pub id: CommandId,
    pub name: String,
    pub raw_args: String,
    pub args: String,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("unknown command '/{0}'")]
    Unknown(String),
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: CommandId::Login,
        name: "login",
        usage: "/login [provider]",
        description: "log in to a provider",
    },
    CommandSpec {
        id: CommandId::Logout,
        name: "logout",
        usage: "/logout [provider]",
        description: "delete provider credentials",
    },
    CommandSpec {
        id: CommandId::Model,
        name: "model",
        usage: "/model [model]",
        description: "show or switch model",
    },
    CommandSpec {
        id: CommandId::Resume,
        name: "resume",
        usage: "/resume [id]",
        description: "show resume help",
    },
    CommandSpec {
        id: CommandId::Config,
        name: "config",
        usage: "/config",
        description: "show configuration",
    },
    CommandSpec {
        id: CommandId::Exit,
        name: "exit",
        usage: "/exit",
        description: "quit rho",
    },
];

pub fn command_prefix(input: &str) -> Option<&str> {
    let token_end = input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len());
    let prefix = input[..token_end].strip_prefix('/')?;
    if prefix.starts_with('/') {
        None
    } else {
        Some(prefix)
    }
}

pub fn matching_commands(prefix: &str) -> Vec<&'static CommandSpec> {
    let prefix = prefix
        .strip_prefix('/')
        .unwrap_or(prefix)
        .to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(&prefix))
        .collect()
}

pub fn parse_command(input: &str) -> Result<Option<CommandInvocation>, CommandParseError> {
    let input = input.trim_end();
    let Some(rest) = input.strip_prefix('/') else {
        return Ok(None);
    };
    if rest.starts_with('/') {
        return Ok(None);
    }

    let name_end = rest
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    let raw_args = rest[name_end..].to_string();
    let args = raw_args.trim().to_string();

    let spec = COMMANDS
        .iter()
        .find(|command| command.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| CommandParseError::Unknown(name.to_string()))?;

    Ok(Some(CommandInvocation {
        id: spec.id,
        name: spec.name.to_string(),
        raw_args,
        args,
    }))
}

pub fn complete_command(input: &str, cursor: usize, spec: &CommandSpec) -> (String, usize) {
    let token_end = first_token_end_byte(input);
    let token_len = input[..token_end].chars().count();
    let args = input[token_end..].trim_start();
    let completed_prefix = format!("/{} ", spec.name);
    let completed_prefix_len = completed_prefix.chars().count();
    let completed = if args.is_empty() {
        completed_prefix
    } else {
        format!("{completed_prefix}{args}")
    };

    let new_cursor = if cursor <= token_len {
        completed_prefix_len
    } else {
        completed
            .chars()
            .count()
            .min(completed_prefix_len.saturating_add(cursor.saturating_sub(token_len)))
    };

    (completed, new_cursor)
}

fn first_token_end_byte(input: &str) -> usize {
    input
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(input.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_all_commands_for_empty_slash_prefix() {
        let matches = matching_commands(command_prefix("/").unwrap());

        assert_eq!(matches.len(), COMMANDS.len());
        assert!(matches.iter().any(|command| command.name == "model"));
    }

    #[test]
    fn additional_leading_slashes_are_literal_text() {
        assert_eq!(command_prefix("//"), None);
        assert_eq!(parse_command("//literal").unwrap(), None);
    }

    #[test]
    fn slash_must_be_first_character_to_parse_as_command() {
        assert_eq!(command_prefix(" /model"), None);
        assert_eq!(parse_command(" /model").unwrap(), None);
    }

    #[test]
    fn matches_commands_by_case_insensitive_prefix() {
        let matches = matching_commands(command_prefix("/M").unwrap());

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "model");
    }

    #[test]
    fn matches_full_command_name() {
        let matches = matching_commands(command_prefix("/model").unwrap());

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, CommandId::Model);
    }

    #[test]
    fn matching_unknown_command_returns_no_matches() {
        let matches = matching_commands(command_prefix("/nope").unwrap());

        assert!(matches.is_empty());
    }

    #[test]
    fn parses_model_command_with_arguments() {
        let invocation = parse_command("/model gpt-5.5").unwrap().unwrap();

        assert_eq!(invocation.id, CommandId::Model);
        assert_eq!(invocation.name, "model");
        assert_eq!(invocation.raw_args, " gpt-5.5");
        assert_eq!(invocation.args, "gpt-5.5");
    }

    #[test]
    fn parses_non_command_as_none() {
        assert_eq!(parse_command("hello /model").unwrap(), None);
    }

    #[test]
    fn rejects_unknown_command() {
        let err = parse_command("/nope").unwrap_err();

        assert_eq!(err, CommandParseError::Unknown("nope".into()));
    }

    #[test]
    fn parses_logout_command_with_provider_argument() {
        let invocation = parse_command("/logout openai-codex").unwrap().unwrap();

        assert_eq!(invocation.id, CommandId::Logout);
        assert_eq!(invocation.args, "openai-codex");
    }

    #[test]
    fn login_usage_accepts_provider_argument() {
        let login = COMMANDS
            .iter()
            .find(|command| command.name == "login")
            .unwrap();

        assert_eq!(login.usage, "/login [provider]");
    }

    #[test]
    fn completes_command_and_preserves_args() {
        let spec = COMMANDS
            .iter()
            .find(|command| command.name == "model")
            .unwrap();
        let (input, cursor) = complete_command("/m gpt-5.5", 2, spec);

        assert_eq!(input, "/model gpt-5.5");
        assert_eq!(cursor, 7);
    }
}
