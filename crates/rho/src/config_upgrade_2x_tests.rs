use std::path::Path;

use pretty_assertions::assert_eq;
use rho_providers::credentials::{CredentialStoreBackend, WebSearchCredential};

use super::{upgrade_2x, Upgraded};
use crate::{
    config::{Config, StreamingMode},
    keybindings::{EditingMode, Keybindings},
    permission::PermissionMode,
    tui::TuiSettings,
};

/// A `config.toml` as rho 2.x saved it: every group, every key.
const SAVED_BY_2X: &str = r#"[model]
provider = "openai"
model = "gpt-5.5"
auth = "api-key"
reasoning = "medium"
fast_mode = false
favorite_models = []

[display]
show_reasoning_output = false
zen_mode = true
output_streaming = "paragraph"
theme = "nord"
max_tool_output_lines = 25
prompt_history_limit = 500
cache_miss_notices = true
show_header_hints = false
notifications = false

[output]
max_output_bytes = 51200

[compaction]
auto_compact = true
compact_threshold_percent = 80
compact_target_percent = 40

[web_search]
mode = "auto"
backend = "openai"

[behavior]
check_for_updates = false
enable_subagents = true
agent_concurrency = 10
advisor_mode = false
workspace_rewind = true
keep_cache_warm = false
edit_tool = "auto"
permission_mode = "plan"
credential_store = "file"
rtk = true
inline_shell = "bash"

[keybindings]
editing_mode = "vim"
search_prompt_history = "ctrl+r"
search_transcript = "ctrl+f"
open_editor = "ctrl+g"
jump_to_bottom = "ctrl+end"
toggle_tool_output = "ctrl+o"
cycle_streaming_mode = "alt+s"
cycle_permission_mode = "alt+m"
insert_newline = "ctrl+j"
queue_prompt = "alt+q"
paste_image = "ctrl+v"
edit_pending_input = "alt+up"
manage_pending_input = "alt+p"
cycle_pinned_model = "ctrl+p"
cycle_pinned_model_back = "ctrl+shift+p"
undo = "ctrl+u"
redo = "ctrl+shift+z"

[prompt_templates]
"#;

fn write(dir: &Path, relative: &str, text: &str) {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(dir: &Path, relative: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(relative)).ok()
}

/// Upgrades the home at `dir`, failing the test if it holds secrets.
fn upgrade(dir: &Path) -> Option<Upgraded> {
    upgrade_2x(&dir.join("config.toml"), dir, |_, _| {
        panic!("this home holds no plaintext key")
    })
    .unwrap()
}

// Covers: a home rho 2.x saved loads on 3.0, with the display, update-check
// and keybinding values it held read from their 3.0 files, the original
// kept, and nothing left for a second start to do.
// Owner: config upgrade
#[test]
fn a_home_saved_by_2x_loads_with_its_settings() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "config.toml", SAVED_BY_2X);

    let upgraded = upgrade(dir.path()).expect("the file holds 2.x keys");

    let mut keybindings = toml::from_str::<Keybindings>(
        "queue_prompt = \"alt+q\"\nmanage_pending_input = \"alt+p\"\nundo = \"ctrl+u\"\n",
    )
    .unwrap();
    keybindings.editing_mode = EditingMode::Vim;
    let config = Config::load(Some(dir.path().join("config.toml"))).unwrap();
    assert_eq!(
        (
            TuiSettings::load(dir.path()).unwrap(),
            config.permission_mode,
            config.credential_store,
            std::fs::read_to_string(&upgraded.backup).unwrap(),
            upgraded.backup,
        ),
        (
            TuiSettings {
                zen_mode: true,
                show_reasoning_output: false,
                output_streaming: StreamingMode::Paragraph,
                show_header_hints: false,
                notifications: false,
                cache_miss_notices: true,
                max_tool_output_lines: 25,
                prompt_history_limit: 500,
                check_for_updates: false,
                theme: "nord".into(),
                keybindings,
            },
            PermissionMode::Plan,
            Some(CredentialStoreBackend::File),
            SAVED_BY_2X.to_owned(),
            dir.path().join("config.toml.2x.bak"),
        )
    );

    let after_first = read(dir.path(), "config.toml");
    assert_eq!(
        (upgrade(dir.path()), read(dir.path(), "config.toml")),
        (None, after_first)
    );
}

// Covers: a value the 3.0 files already hold is not overwritten by the 2.x
// one and is reported under the 2.x key the user wrote, unless both agree;
// a file the upgrade rewrites is copied first, and an older backup is never
// replaced.
// Owner: config upgrade
#[test]
fn values_in_the_3_0_files_win_and_rewritten_files_are_backed_up() {
    let dir = tempfile::tempdir().unwrap();
    let config = "[display]\ntheme = \"nord\"\nzen_mode = false\nshow_header_hints = false\n\n[keybindings]\nundo = \"ctrl+u\"\nredo = \"ctrl+y\"\n";
    write(dir.path(), "config.toml", config);
    write(dir.path(), "config.toml.2x.bak", "an earlier backup\n");
    // `redo` is as an interrupted upgrade left it: already the 2.x value.
    write(
        dir.path(),
        "tui.toml",
        "theme = \"dracula\"\n\n[keybindings]\nundo = \"ctrl+x\"\nredo = \"ctrl+y\"\n",
    );
    write(dir.path(), "clients/tui.toml", "# mine\nzen_mode = true\n");

    let upgraded = upgrade(dir.path()).expect("the file holds 2.x keys");

    assert_eq!(
        (
            upgraded.already_set,
            upgraded.backup,
            read(dir.path(), "config.toml.2x.bak.1"),
            read(dir.path(), "config.toml.2x.bak"),
            read(dir.path(), "tui.toml"),
            read(dir.path(), "tui.toml.2x.bak"),
            read(dir.path(), "clients/tui.toml"),
            read(dir.path(), "clients/tui.toml.2x.bak"),
        ),
        (
            vec![
                "display.zen_mode".to_owned(),
                "display.theme".to_owned(),
                "keybindings.undo".to_owned(),
            ],
            dir.path().join("config.toml.2x.bak.1"),
            Some(config.to_owned()),
            Some("an earlier backup\n".to_owned()),
            Some(
                "theme = \"dracula\"\n\n[keybindings]\nundo = \"ctrl+x\"\nredo = \"ctrl+y\"\n"
                    .to_owned()
            ),
            None,
            Some("# mine\nzen_mode = true\nshow_header_hints = false\n".to_owned()),
            Some("# mine\nzen_mode = true\n".to_owned()),
        )
    );
}

// Covers: plaintext web-search keys go to the store the file selects, and a
// store that refuses them does not stop the upgrade or leave them in the
// 3.0 file.
// Owner: config upgrade
#[test]
fn a_refused_web_search_key_does_not_stop_the_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "config.toml",
        "web_search_exa_api_key = \"exa-key\"\n\n[behavior]\ncredential_store = \"file\"\n",
    );

    let mut asked = None;
    let upgraded = upgrade_2x(
        &dir.path().join("config.toml"),
        dir.path(),
        |backend, keys| {
            asked = Some((backend, keys.to_vec()));
            anyhow::bail!("store is locked")
        },
    )
    .unwrap()
    .expect("the file holds 2.x keys");

    assert_eq!(
        (
            asked,
            upgraded.secrets_error,
            read(dir.path(), "config.toml"),
        ),
        (
            Some((
                Some(CredentialStoreBackend::File),
                vec![(WebSearchCredential::Exa, "exa-key".to_owned())],
            )),
            Some("store is locked".to_owned()),
            Some("\n[behavior]\ncredential_store = \"file\"\n".to_owned()),
        )
    );
}

// Covers: only 2.x keys are rewritten. A key neither version knows is left
// for loading to refuse, in a 3.0 file and in a 2.x one alike.
// Owner: config upgrade
#[test]
fn unknown_keys_still_fail_the_load() {
    for (name, config, upgrades) in [
        ("3.0 file", "[behavior]\npermision_mode = \"plan\"\n", false),
        (
            "2.x file",
            "[display]\nzen_mode = true\n\n[behavior]\npermision_mode = \"plan\"\n",
            true,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "config.toml", config);
        assert_eq!(upgrade(dir.path()).is_some(), upgrades, "{name}");
        let error = Config::load(Some(dir.path().join("config.toml"))).unwrap_err();
        assert_eq!(
            error.root_cause().to_string(),
            "unknown key `behavior.permision_mode`",
            "{name}"
        );
    }
}
