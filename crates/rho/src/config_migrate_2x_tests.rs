use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use rho_providers::credentials::WebSearchCredential;
use toml_edit::DocumentMut;

use super::{migrate_2x, Change, MOVES};

struct Case {
    name: &'static str,
    before: &'static str,
    agent: &'static str,
    client_tui: &'static str,
    tui_local: &'static str,
    secrets: &'static [(WebSearchCredential, &'static str)],
}

const CASES: &[Case] = &[
    Case {
        name: "flat agent keys move into their groups",
        before: r#"
provider = "anthropic"
model = "claude-sonnet-4-5"
auth = "anthropic-api-key"
reasoning = "high"
fast_mode = true
favorite_models = ["openai/gpt-5.5"]
max_output_bytes = 4096
auto_compact = false
compact_threshold_percent = 70
compact_target_percent = 30
enable_subagents = false
permission_mode = "plan"
rtk = false
inline_shell = "zsh"
"#,
        agent: r#"
[model]
provider = "anthropic"
model = "claude-sonnet-4-5"
auth = "anthropic-api-key"
reasoning = "high"
fast_mode = true
favorite_models = ["openai/gpt-5.5"]

[output]
max_output_bytes = 4096

[compaction]
auto_compact = false
compact_threshold_percent = 70
compact_target_percent = 30

[behavior]
enable_subagents = false
permission_mode = "plan"
rtk = false
inline_shell = "zsh"
"#,
        client_tui: "",
        tui_local: "",
        secrets: &[],
    },
    Case {
        name: "client settings, theme, and chords leave config.toml",
        before: r#"
show_reasoning_output = false
zen_mode = true
max_tool_output_lines = 20
check_for_updates = false

[display]
output_streaming = "off"
prompt_history_limit = 50
cache_miss_notices = true
show_header_hints = false
notifications = false
theme = "dracula"

[keybindings]
editing_mode = "vim"
open_editor = "ctrl+e"
"#,
        agent: "",
        client_tui: r#"
show_reasoning_output = false
zen_mode = true
max_tool_output_lines = 20
check_for_updates = false
output_streaming = "off"
prompt_history_limit = 50
cache_miss_notices = true
show_header_hints = false
notifications = false
editing_mode = "vim"
"#,
        tui_local: r#"
theme = "dracula"

[keybindings]
open_editor = "ctrl+e"
"#,
        secrets: &[],
    },
    Case {
        name: "a group key wins over its flat spelling",
        before: r#"
reasoning = "low"
show_reasoning_output = true
zen_mode = true
max_tool_output_lines = 5
check_for_updates = true
permission_mode = "plan"

[model]
reasoning = "medium"

[display]
show_reasoning_output = false
zen_mode = false
max_tool_output_lines = 30

[behavior]
check_for_updates = false
permission_mode = "auto"
"#,
        agent: r#"
[model]
reasoning = "medium"

[behavior]
permission_mode = "auto"
"#,
        client_tui: r#"
show_reasoning_output = false
zen_mode = false
max_tool_output_lines = 30
check_for_updates = false
"#,
        tui_local: "",
        secrets: &[],
    },
    Case {
        name: "title keys become the session-title agent",
        before: r#"
reasoning_effort = "low"
title_provider = "openai"
title_model = "gpt-5.5-mini"

[title]
auth = "api-key"
"#,
        agent: r#"
[model]
reasoning = "low"

[internal_agents.session-title]
provider = "openai"
model = "gpt-5.5-mini"
auth = "api-key"
"#,
        client_tui: "",
        tui_local: "",
        secrets: &[],
    },
    Case {
        name: "a session-title entry wins over title keys",
        before: r#"
title_model = "gpt-5.5-mini"

[internal_agents.session-title]
model = "gpt-5.5"
"#,
        agent: r#"
[internal_agents.session-title]
model = "gpt-5.5"
"#,
        client_tui: "",
        tui_local: "",
        secrets: &[],
    },
    Case {
        name: "web search moves to mode and backend, and keys come out",
        before: r#"
web_search_provider = "exa"
web_search_openai_api_key = " sk-flat "

[web_search]
hosted = false
exa_api_key = "exa-key"
brave_api_key = ""
"#,
        agent: r#"
[web_search]
mode = "backend"
backend = "exa"
"#,
        client_tui: "",
        tui_local: "",
        secrets: &[
            (WebSearchCredential::Exa, "exa-key"),
            (WebSearchCredential::OpenAi, "sk-flat"),
        ],
    },
    Case {
        name: "native-only web search turns off instead of enabling a backend",
        before: r#"
[web_search]
hosted = true
provider = "disabled"
"#,
        agent: r#"
[web_search]
mode = "off"
"#,
        client_tui: "",
        tui_local: "",
        secrets: &[],
    },
    Case {
        name: "hashline becomes str_replace and the retired rewind key goes",
        before: r#"
[behavior]
edit_tool = "hashline"
experimental_workspace_rewind = false

[providers.custom.local]
base_url = "http://127.0.0.1:8080/v1"
edit_tool = "hashline"
"#,
        agent: r#"
[behavior]
edit_tool = "str_replace"

[providers.custom.local]
base_url = "http://127.0.0.1:8080/v1"
edit_tool = "str_replace"
"#,
        client_tui: "",
        tui_local: "",
        secrets: &[],
    },
];

fn table(doc: &DocumentMut) -> toml::Table {
    toml::from_str(&doc.to_string()).unwrap()
}

fn expected(text: &str) -> toml::Table {
    toml::from_str(text).unwrap()
}

// Covers: every 2.x key lands at its 3.0 key in the right file, with the
// value 2.x loaded, and every MOVES row has a fixture that moves it.
// Owner: config migration
#[test]
fn migrate_2x_moves_every_key() {
    let mut moved = BTreeSet::new();
    for case in CASES {
        let migrated = migrate_2x(case.before.parse().unwrap());
        assert_eq!(
            (
                table(&migrated.agent),
                table(&migrated.client_tui),
                table(&migrated.tui_local),
                migrated.secrets,
            ),
            (
                expected(case.agent),
                expected(case.client_tui),
                expected(case.tui_local),
                case.secrets
                    .iter()
                    .map(|(credential, secret)| (*credential, (*secret).to_owned()))
                    .collect::<Vec<_>>(),
            ),
            "{}",
            case.name
        );
        for change in migrated.report {
            if let Change::Moved { from, .. } = change {
                moved.insert(from);
            }
        }
    }
    assert_eq!(
        MOVES
            .iter()
            .map(|(from, _, _)| *from)
            .filter(|from| !moved.contains(*from))
            .collect::<Vec<_>>(),
        Vec::<&str>::new()
    );
}

// Covers: keys that did not move keep their comments and place.
// Owner: config migration
#[test]
fn migrate_2x_keeps_comments_on_keys_that_stay() {
    let migrated = migrate_2x(
        "# my setup\n[behavior]\n# never ask\npermission_mode = \"bypass\" # yolo\ncheck_for_updates = false\n"
            .parse()
            .unwrap(),
    );
    assert_eq!(
        migrated.agent.to_string(),
        "# my setup\n[behavior]\n# never ask\npermission_mode = \"bypass\" # yolo\n"
    );
}
