use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandId {
    Advisor,
    New,
    Login,
    Logout,
    Model,
    RefreshModels,
    Resume,
    Rewind,
    Sessions,
    Tree,
    Config,
    Info,
    Help,
    Compact,
    Copy,
    Goal,
    Skills,
    Theme,
    Hooks,
    Agents,
    CreateAgent,
    Attach,
    Changelog,
    Diff,
    Doctor,
    Limits,
    Export,
    Mcp,
    Title,
    Fast,
    Workflow,
    Side,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
    pub argument_choices: &'static [CommandArgumentChoice],
}

fn agents_create_request(args: &str) -> Option<&str> {
    let mut parts = args.splitn(2, char::is_whitespace);
    let action = parts.next()?;
    action
        .eq_ignore_ascii_case("create")
        .then(|| parts.next().unwrap_or_default().trim())
}

/// User request for `/agents create` / `/create-agent` after the command token.
///
/// `expanded_args` is the text after the first slash token in the expanded
/// model input, so pasted bodies stay intact. `/agents create` still has to
/// drop the `create` sub-action; `/create-agent` does not.
pub(crate) fn create_agent_request<'a>(command_name: &str, expanded_args: &'a str) -> &'a str {
    let args = expanded_args.trim();
    if command_name.eq_ignore_ascii_case("agents") {
        agents_create_request(args).unwrap_or("")
    } else {
        args
    }
}

impl CommandSpec {
    /// A slash name that only resolves to `target`. No unique id or handler.
    const fn alias(
        name: &'static str,
        usage: &'static str,
        description: &'static str,
        target: CommandId,
    ) -> Self {
        Self {
            id: target,
            name,
            usage,
            description,
            argument_choices: &[],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandArgumentChoice {
    pub completion: &'static str,
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

const GOAL_ARGUMENT_CHOICES: &[CommandArgumentChoice] = &[
    CommandArgumentChoice {
        completion: "/goal resume",
        usage: "/goal resume",
        description: "verify and continue a blocked goal",
    },
    CommandArgumentChoice {
        completion: "/goal clear",
        usage: "/goal clear",
        description: "stop and clear the current goal",
    },
];

const AGENTS_ARGUMENT_CHOICES: &[CommandArgumentChoice] = &[CommandArgumentChoice {
    completion: "/agents create",
    usage: "/agents create [request]",
    description: "create an agent through a guided questionnaire",
}];

const ADVISOR_ARGUMENT_CHOICES: &[CommandArgumentChoice] = &[
    CommandArgumentChoice {
        completion: "/advisor on",
        usage: "/advisor on",
        description: "let the agent ask an advisor model for guidance",
    },
    CommandArgumentChoice {
        completion: "/advisor off",
        usage: "/advisor off",
        description: "work without advisor guidance",
    },
];

const FAST_ARGUMENT_CHOICES: &[CommandArgumentChoice] = &[
    CommandArgumentChoice {
        completion: "/fast on",
        usage: "/fast on",
        description: "enable faster Codex responses at higher credit cost",
    },
    CommandArgumentChoice {
        completion: "/fast off",
        usage: "/fast off",
        description: "use standard Codex response speed",
    },
];

const CHANGELOG_ARGUMENT_CHOICES: &[CommandArgumentChoice] = &[CommandArgumentChoice {
    completion: "/changelog latest",
    usage: "/changelog latest",
    description: "fetch notes for the latest published release",
}];

// Keep alphabetical by `name` so the slash palette stays sorted as commands are added.
pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: CommandId::Advisor,
        name: "advisor",
        usage: "/advisor [on|off]",
        description: "toggle advisor mode, which reviews the session with a second model",
        argument_choices: ADVISOR_ARGUMENT_CHOICES,
    },
    CommandSpec {
        id: CommandId::Agents,
        name: "agents",
        usage: "/agents [create]",
        description: "browse agents or create one through a guided questionnaire",
        argument_choices: AGENTS_ARGUMENT_CHOICES,
    },
    CommandSpec {
        id: CommandId::Attach,
        name: "attach",
        usage: "/attach",
        description: "attach to a subagent run",
        argument_choices: &[],
    },
    CommandSpec::alias("btw", "/btw [prompt]", "alias for /side", CommandId::Side),
    CommandSpec {
        id: CommandId::Changelog,
        name: "changelog",
        usage: "/changelog [latest]",
        description: "show release notes for this install, or the latest published release",
        argument_choices: CHANGELOG_ARGUMENT_CHOICES,
    },
    CommandSpec::alias("clear", "/clear", "alias for /new", CommandId::New),
    CommandSpec {
        id: CommandId::Compact,
        name: "compact",
        usage: "/compact",
        description: "compact older conversation context",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Config,
        name: "config",
        usage: "/config",
        description: "open configuration picker",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Copy,
        name: "copy",
        usage: "/copy",
        description: "copy the last assistant message to the clipboard",
        argument_choices: &[],
    },
    CommandSpec::alias(
        "create-agent",
        "/create-agent [request]",
        "alias for /agents create",
        CommandId::CreateAgent,
    ),
    CommandSpec {
        id: CommandId::Diff,
        name: "diff",
        usage: "/diff",
        description: "show Git status and worktree patches",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Doctor,
        name: "doctor",
        usage: "/doctor",
        description: "run local setup diagnostics",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Exit,
        name: "exit",
        usage: "/exit",
        description: "quit rho",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Export,
        name: "export",
        usage: "/export [path]",
        description: "export the session transcript (html/md/json; default ~/.rho/exports)",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Fast,
        name: "fast",
        usage: "/fast [on|off]",
        description: "toggle faster Codex responses at higher credit cost",
        argument_choices: FAST_ARGUMENT_CHOICES,
    },
    CommandSpec {
        id: CommandId::Goal,
        name: "goal",
        usage: "/goal [condition|resume|clear]",
        description: "show status or work until a condition is met",
        argument_choices: GOAL_ARGUMENT_CHOICES,
    },
    CommandSpec {
        id: CommandId::Help,
        name: "help",
        usage: "/help",
        description: "show keyboard shortcuts",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Hooks,
        name: "hooks",
        usage: "/hooks",
        description: "reload lifecycle hooks and show what each one will run",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Info,
        name: "info",
        usage: "/info",
        description: "show runtime, usage, and workspace details",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Limits,
        name: "limits",
        usage: "/limits",
        description: "show connected usage limits in an overlay",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Login,
        name: "login",
        usage: "/login [provider]",
        description: "log in to a provider",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Logout,
        name: "logout",
        usage: "/logout [provider]",
        description: "delete provider credentials",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Mcp,
        name: "mcp",
        usage: "/mcp",
        description: "show configured MCP servers and session load status",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Model,
        name: "model",
        usage: "/model [model]",
        description: "show or switch model",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::New,
        name: "new",
        usage: "/new",
        description: "start a new session",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::RefreshModels,
        name: "refresh-models",
        usage: "/refresh-models",
        description: "refresh cached provider model lists and the models.dev catalog",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Resume,
        name: "resume",
        usage: "/resume [id]",
        description: "resume a saved session",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Rewind,
        name: "rewind",
        usage: "/rewind [turn]",
        description: "preview and restore a completed turn's workspace checkpoint",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Sessions,
        name: "sessions",
        usage: "/sessions",
        description: "browse, resume, and delete saved sessions in every directory",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Side,
        name: "side",
        usage: "/side [prompt]",
        description: "ask aside without changing the session",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Skills,
        name: "skills",
        usage: "/skills",
        description: "show loaded skills and descriptions",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Theme,
        name: "theme",
        usage: "/theme",
        description: "preview and apply a color theme",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Title,
        name: "title",
        usage: "/title <name>",
        description: "rename the current session",
        argument_choices: &[],
    },
    CommandSpec {
        id: CommandId::Tree,
        name: "tree",
        usage: "/tree",
        description: "navigate this session's conversation tree",
        argument_choices: &[],
    },
    CommandSpec::alias("usage", "/usage", "alias for /limits", CommandId::Limits),
    CommandSpec {
        id: CommandId::Workflow,
        name: "workflow",
        usage: "/workflow",
        description: "start a workflow or check runs",
        argument_choices: &[],
    },
];

pub(crate) fn argument_choices(input: &str, cursor: usize) -> &'static [CommandArgumentChoice] {
    let cursor_byte = input
        .char_indices()
        .nth(cursor)
        .map(|(index, _)| index)
        .unwrap_or(input.len());
    let (before_cursor, after_cursor) = input.split_at(cursor_byte);
    if !after_cursor.is_empty() {
        return &[];
    }
    let Some((command, args)) = before_cursor.split_once(char::is_whitespace) else {
        return &[];
    };
    let command = command.strip_prefix('/').unwrap_or(command);
    let Some(spec) = COMMANDS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(command))
    else {
        return &[];
    };
    if args.trim().is_empty() {
        spec.argument_choices
    } else {
        &[]
    }
}

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
    // COMMANDS is kept alphabetical by name; filtering preserves that order.
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(&prefix))
        .collect()
}

pub fn parse_command(input: &str) -> Result<Option<CommandInvocation>, CommandParseError> {
    // Trailing newlines are paste-burst Enter artifacts, not a multiline
    // prompt. Strip them before the newline check so `/exit\n` still quits.
    let input = input.trim_end();
    if input.contains(['\n', '\r']) {
        return Ok(None);
    }
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

    Ok(Some(canonicalize_invocation(CommandInvocation {
        id: spec.id,
        name: spec.name.to_string(),
        raw_args,
        args,
    })))
}

fn canonicalize_invocation(mut invocation: CommandInvocation) -> CommandInvocation {
    if invocation.id == CommandId::Agents {
        if let Some(request) = agents_create_request(&invocation.args) {
            invocation.id = CommandId::CreateAgent;
            invocation.args = request.to_string();
        }
    }
    invocation
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

pub(crate) fn complete_argument_choice(choice: &CommandArgumentChoice) -> (String, usize) {
    let input = choice.completion.to_string();
    let cursor = input.chars().count();
    (input, cursor)
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
        let matches = matching_commands(command_prefix("/Mo").unwrap());

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "model");
    }

    #[test]
    fn configuration_commands_are_not_in_the_command_palette() {
        for name in [
            "title-model",
            "refresh-model-list",
            "auto",
            "plan",
            "supervised",
        ] {
            assert!(matching_commands(name).is_empty());
            assert_eq!(
                parse_command(&format!("/{name}")),
                Err(CommandParseError::Unknown(name.into()))
            );
        }
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
    fn multiline_slash_text_is_not_a_command() {
        assert_eq!(parse_command("/model\ngpt-5.5").unwrap(), None);
        assert_eq!(parse_command("/model\r\ngpt-5.5").unwrap(), None);
    }

    // Covers: a paste-burst Enter that became a trailing newline must not
    // demote `/exit` (and other slash commands) into a prompt.
    // Owner: command parser
    #[test]
    fn trailing_newline_still_parses_as_command() {
        let invocation = parse_command("/exit\n").unwrap().unwrap();
        assert_eq!(invocation.id, CommandId::Exit);
        assert_eq!(
            parse_command("/exit\r\n").unwrap().unwrap().id,
            CommandId::Exit
        );
        assert_eq!(
            parse_command("/model gpt-5.5\n").unwrap().unwrap().id,
            CommandId::Model
        );
    }

    #[test]
    fn rejects_unknown_command() {
        for input in ["/nope", "/nope\n", "/nope\r\n"] {
            assert_eq!(
                parse_command(input),
                Err(CommandParseError::Unknown("nope".into()))
            );
        }
    }

    #[test]
    fn parses_side_command_with_arguments() {
        let invocation = parse_command("/side what is this lock").unwrap().unwrap();

        assert_eq!(invocation.id, CommandId::Side);
        assert_eq!(invocation.name, "side");
        assert_eq!(invocation.args, "what is this lock");
    }

    #[test]
    fn parses_title_command_with_arguments() {
        let invocation = parse_command("/title My Session").unwrap().unwrap();

        assert_eq!(invocation.id, CommandId::Title);
        assert_eq!(invocation.name, "title");
        assert_eq!(invocation.raw_args, " My Session");
        assert_eq!(invocation.args, "My Session");
    }

    // Covers: the palette lists commands in name order, which decides what the
    // unfiltered short list shows first.
    // Owner: command table
    #[test]
    fn command_palette_stays_alphabetical() {
        let names = COMMANDS.iter().map(|spec| spec.name).collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort_unstable();

        assert_eq!(names, sorted);
    }

    // Covers: alias names must dispatch as their target command and stay
    // discoverable by prefix in the palette.
    // Owner: command table
    #[test]
    fn aliases_resolve_to_target_commands() {
        let cases = [
            ("/clear", CommandId::New, "clear"),
            ("/CLEAR", CommandId::New, "clear"),
            ("/usage", CommandId::Limits, "usage"),
            ("/create-agent", CommandId::CreateAgent, "create-agent"),
            ("/btw", CommandId::Side, "btw"),
            ("/BTW", CommandId::Side, "btw"),
        ];
        for (input, id, name) in cases {
            let invocation = parse_command(input).unwrap().unwrap();
            assert_eq!(invocation.id, id);
            assert_eq!(invocation.name, name);
        }

        assert!(matching_commands("cl")
            .iter()
            .any(|command| command.name == "clear"));
        assert!(matching_commands("us")
            .iter()
            .any(|command| command.name == "usage"));
        assert!(matching_commands("bt")
            .iter()
            .any(|command| command.name == "btw"));
    }

    // Covers: both documented spellings must canonicalize to CreateAgent while
    // bare /agents continues to open the catalog.
    // Owner: command parser
    #[test]
    fn canonicalizes_agent_creation_commands() {
        let cases = [
            ("/agents create", CommandId::CreateAgent, ""),
            (
                "/agents CREATE a read-only reviewer",
                CommandId::CreateAgent,
                "a read-only reviewer",
            ),
            ("/create-agent", CommandId::CreateAgent, ""),
            (
                "/create-agent a planner",
                CommandId::CreateAgent,
                "a planner",
            ),
            ("/agents", CommandId::Agents, ""),
        ];

        for (input, id, args) in cases {
            let invocation = parse_command(input).unwrap().unwrap();
            assert_eq!(invocation.id, id, "{input}");
            assert_eq!(invocation.args, args, "{input}");
        }
    }

    // Covers: the execute path must strip `create` from `/agents create`
    // while leaving `/create-agent` request text unchanged.
    // Owner: command parser
    #[test]
    fn create_agent_request_matches_both_spellings() {
        assert_eq!(
            create_agent_request("agents", "create a read-only reviewer"),
            "a read-only reviewer"
        );
        assert_eq!(create_agent_request("agents", "create"), "");
        assert_eq!(
            create_agent_request("agents", "CREATE a read-only reviewer"),
            "a read-only reviewer"
        );
        assert_eq!(
            create_agent_request("create-agent", "a planner"),
            "a planner"
        );
        assert_eq!(
            create_agent_request("create-agent", "create a planner"),
            "create a planner"
        );
        assert_eq!(create_agent_request("create-agent", ""), "");
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
