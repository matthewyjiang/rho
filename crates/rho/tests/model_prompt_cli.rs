//! CLI/editor process coverage. Filesystem transaction rules live beside the editor store.
#![cfg(unix)]

use pretty_assertions::assert_eq;
use rho_tui_pty::{Key, PtyHarness, PtySize, RhoLaunchPlan, WaitTimeout};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    editor: PathBuf,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".rho")).unwrap();
        fs::write(
            root.path().join(".rho/config.toml"),
            r#"[model]
provider = "openai-codex"
model = "gpt-6-astra"
auth = "codex"
[model.aliases]
local = "ollama/team/model:q8"
"#,
        )
        .unwrap();
        let editor = root.path().join("test editor.sh");
        fs::write(
            &editor,
            format!("printf '%s' \"$1\" > \"$EDITOR_DRAFT_RECORD\"\n{script}\n"),
        )
        .unwrap();
        Self { root, editor }
    }

    fn command(&self) -> Command {
        self.command_with_args(&["model-prompt", "edit"])
    }

    fn command_with_args(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.path())
            .env("PATH", "/usr/bin:/bin")
            .env(
                "VISUAL",
                format!(
                    "/bin/sh {}",
                    shell_words::quote(self.editor.to_str().unwrap())
                ),
            )
            .env("EDITOR", "/nonexistent-editor-visual-must-win")
            .env("EDITOR_DRAFT_RECORD", self.root.path().join("draft-path"))
            .current_dir(self.root.path())
            .stdin(Stdio::null());
        command
    }

    fn directory(&self) -> PathBuf {
        self.root.path().join(".rho/model-prompts")
    }
    fn draft(&self) -> PathBuf {
        PathBuf::from(fs::read_to_string(self.root.path().join("draft-path")).unwrap())
    }

    fn config(&self) -> Vec<u8> {
        fs::read(self.root.path().join(".rho/config.toml")).unwrap()
    }

    fn picker(&self, args: &[&str]) -> PtyHarness {
        let command = self.command_with_args(args);
        self.picker_command(&command)
    }

    fn picker_command(&self, command: &Command) -> PtyHarness {
        let plan = RhoLaunchPlan {
            binary: PathBuf::from(env!("CARGO_BIN_EXE_rho")),
            size: PtySize {
                rows: 28,
                cols: 100,
            },
            args: command
                .get_args()
                .map(|arg| arg.to_str().unwrap().to_owned())
                .collect(),
            env: command
                .get_envs()
                .filter_map(|(key, value)| {
                    value.map(|value| {
                        (
                            key.to_str().unwrap().to_owned(),
                            value.to_str().unwrap().to_owned(),
                        )
                    })
                })
                .chain([("TERM".into(), "xterm-256color".into())])
                .collect(),
            cwd: self.root.path().to_path_buf(),
        };
        PtyHarness::spawn_named(&plan, "model_prompt_edit").unwrap()
    }
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// Covers: early CLI dispatch resolves identity offline, honors VISUAL with quoted
// arguments, seeds new files, and finds renamed files by frontmatter on later edits.
// Owner: CLI process integration.
#[test]
fn edit_resolves_targets_and_reuses_arbitrary_filenames() {
    for (flags, provider, model, filename) in [
        (
            vec!["--model", "gpt-6-astra"],
            "openai-codex",
            "gpt-6-astra",
            "openai-codex_gpt-6-astra.md",
        ),
        (
            vec!["--provider", "ollama", "--model", "team/model:q8"],
            "ollama",
            "team/model:q8",
            "ollama_team-model-q8.md",
        ),
        (
            vec!["--model", "@local"],
            "ollama",
            "team/model:q8",
            "ollama_team-model-q8.md",
        ),
    ] {
        let fixture = Fixture::new("printf 'custom instruction\\n' >> \"$1\"");
        assert_success(fixture.command().args(&flags).output().unwrap());
        let generated = fixture.directory().join(filename);
        let contents = fs::read_to_string(&generated).unwrap();
        let frontmatter: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(contents.split("---").nth(1).unwrap()).unwrap();
        assert_eq!(
            frontmatter,
            serde_yaml_ng::to_value(
                serde_json::json!({"provider": provider, "model": model, "mode": "append"})
            )
            .unwrap()
        );
        let renamed = fixture.directory().join("my-tuning.md");
        fs::rename(&generated, &renamed).unwrap();
        assert_success(fixture.command().args(&flags).output().unwrap());
        assert_eq!(
            fs::read_to_string(renamed).unwrap(),
            format!("{contents}custom instruction\n")
        );
        assert!(!generated.exists());
        assert!(!fixture.draft().exists());
        assert!(!fixture.root.path().join(".rho/sessions").exists());
    }
}

// Covers: unsuccessful edits preserve live content and keep recoverable draft bytes;
// blank creation cancelled unchanged must not poison future session startup.
// Owner: CLI/editor process boundary (store unit tests own collision/concurrency).
#[test]
fn cancelled_invalid_and_failed_editors_do_not_install_bad_files() {
    let original = "---\nprovider: openai-codex\nmodel: gpt-6-astra\nmode: append\n---\noriginal\n";
    for (script, existing, success, recovery) in [
        ("exit 0", false, true, false),
        ("exit 0", true, true, false),
        ("printf 'invalid' > \"$1\"", true, false, true),
        ("printf 'failed draft' > \"$1\"; exit 9", true, false, true),
        ("printf '%s\\n' '---' 'provider: other' 'model: gpt-6-astra' '---' 'changed identity' > \"$1\"", true, false, true),
    ] {
        let fixture = Fixture::new(script);
        let live = fixture.directory().join("selected.md");
        if existing {
            fs::create_dir_all(fixture.directory()).unwrap();
            fs::write(&live, original).unwrap();
        }
        let output = fixture.command().args(["--model", "gpt-6-astra"]).output().unwrap();
        assert_eq!(output.status.success(), success, "{script}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fixture.draft().exists(), recovery);
        assert_eq!(fs::read_to_string(live).ok().as_deref(), existing.then_some(original));
        if !existing {
            assert!(!fixture.directory().join("openai-codex_gpt-6-astra.md").exists());
        }
    }
}

// Covers: no editor or conflicting alias identity must fail without opening a draft.
// Owner: early CLI validation, before filesystem editing.
#[test]
fn invalid_editor_or_selection_does_not_start_editing() {
    for missing_editor in [true, false] {
        let fixture = Fixture::new("exit 0");
        let mut command = fixture.command();
        if missing_editor {
            command
                .args(["--model", "gpt-6-astra"])
                .env_remove("VISUAL")
                .env_remove("EDITOR");
        } else {
            command.args(["--provider", "openai", "--model", "@local"]);
        }
        assert!(!command.output().unwrap().status.success());
        assert!(!fixture.root.path().join("draft-path").exists());
        assert!(!fixture.directory().exists());
    }
}

// Covers: absent identity must not silently edit the configured default when no
// terminal can host selection. Owner: CLI dispatch, before editor/filesystem work.
#[test]
fn nonterminal_missing_model_does_not_start_editing() {
    for flags in [vec![], vec!["--provider", "xai"]] {
        let fixture = Fixture::new("exit 0");
        let config = fixture.config();
        assert!(!fixture
            .command()
            .args(flags)
            .output()
            .unwrap()
            .status
            .success());
        assert!(!fixture.root.path().join("draft-path").exists());
        assert!(!fixture.directory().exists());
        assert_eq!(fixture.config(), config);
    }
}

// Covers: invalid editor settings fail before entering the interactive picker.
// Owner: CLI startup through a real terminal; nonterminal dispatch is covered above.
#[test]
fn invalid_editor_fails_before_picker() {
    for visual in [None, Some("   "), Some("'unterminated")] {
        let fixture = Fixture::new("exit 0");
        let mut command = fixture.command();
        command.env_remove("VISUAL").env_remove("EDITOR");
        if let Some(visual) = visual {
            command.env("VISUAL", visual);
        }
        let mut harness = fixture.picker_command(&command);
        assert_ne!(harness.wait_for_exit(PICKER_WAIT).unwrap(), 0);
        assert_eq!(harness.raw_sequence_occurrences(b"\x1b[?1049h"), 0);
        assert!(!fixture.root.path().join("draft-path").exists());
        assert!(!fixture.directory().exists());
    }
}

// Covers: a global model flag must bypass terminal selection just like the edit
// subcommand flag. Owner: CLI argument propagation.
#[test]
fn top_level_model_bypasses_picker() {
    let fixture = Fixture::new("printf 'global selection\\n' >> \"$1\"");
    let config = fixture.config();
    assert_success(
        fixture
            .command_with_args(&["--model", "@local", "model-prompt", "edit"])
            .output()
            .unwrap(),
    );
    assert!(fixture.directory().join("ollama_team-model-q8.md").exists());
    assert!(!fixture.draft().exists());
    assert_eq!(fixture.config(), config);
}

// Covers: offline filtering and Enter must edit the selected row, not the config
// default, without changing the active model. Owner: standalone picker PTY UX.
#[test]
fn picker_saves_filtered_nondefault_model_without_login() {
    for args in [
        vec!["model-prompt", "edit"],
        vec!["model-prompt", "edit", "--provider", "xai"],
        vec!["--provider", "xai", "model-prompt", "edit"],
    ] {
        let fixture = Fixture::new("printf 'selected instruction\\n' >> \"$1\"");
        let config = fixture.config();
        let mut harness = fixture.picker(&args);
        harness
            .wait_for_text("Select model prompt", PICKER_WAIT)
            .unwrap();
        if args.contains(&"--provider") {
            assert!(!harness.screen().contains_text("openai-codex"));
        }
        harness.paste("grok-4.5").unwrap();
        // The query is a durable signal that the whole paste was processed,
        // unlike a model label which may already be visible before filtering.
        harness.wait_for_text("> grok-4.5", PICKER_WAIT).unwrap();
        harness.inject_key(&Key::Enter).unwrap();
        assert_eq!(harness.wait_for_exit(PICKER_WAIT).unwrap(), 0);
        harness.assert_raw_contains(b"\x1b[?1049l").unwrap();
        let contents = fs::read_to_string(fixture.directory().join("xai_grok-4.5.md")).unwrap();
        let frontmatter: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(contents.split("---").nth(1).unwrap()).unwrap();
        assert_eq!(
            frontmatter,
            serde_yaml_ng::to_value(serde_json::json!({
                "provider": "xai", "model": "grok-4.5", "mode": "append"
            }))
            .unwrap()
        );
        assert!(contents.ends_with("selected instruction\n"));
        let prompts: Vec<_> = fs::read_dir(fixture.directory())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .collect();
        assert_eq!(prompts, vec![fixture.directory().join("xai_grok-4.5.md")]);
        assert!(!fixture.draft().exists());
        assert!(!fixture.draft().parent().unwrap().exists());
        assert_eq!(fixture.config(), config);
        assert!(!fixture.root.path().join(".rho/sessions").exists());
    }
}

// Covers: both cancellation keys must restore the terminal without launching an
// editor or creating prompts/drafts. Owner: standalone picker PTY lifecycle.
#[test]
fn picker_cancel_restores_terminal_without_side_effects() {
    for key in [Key::Esc, Key::Ctrl('c')] {
        let fixture = Fixture::new("exit 9");
        let config = fixture.config();
        let mut harness = fixture.picker(&["model-prompt", "edit"]);
        harness
            .wait_for_text("Select model prompt", PICKER_WAIT)
            .unwrap();
        harness.inject_key(&key).unwrap();
        assert_eq!(harness.wait_for_exit(PICKER_WAIT).unwrap(), 0);
        harness.assert_raw_contains(b"\x1b[?1049l").unwrap();
        harness.assert_raw_contains(b"\x1b[?25h").unwrap();
        harness.assert_raw_contains(b"\x1b[?2004l").unwrap();
        harness.assert_raw_contains(b"\x1b[>4;0m").unwrap();
        harness.assert_raw_contains(b"\x1b[<1u").unwrap();
        assert!(!fixture.root.path().join("draft-path").exists());
        assert_eq!(fixture.config(), config);
        let entries: Vec<_> = fs::read_dir(fixture.root.path().join(".rho"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.toml")]);
    }
}

// Same startup failure bound as the shared PTY scenarios; waits observe output/exit.
const PICKER_WAIT: WaitTimeout = WaitTimeout::secs(20, "model prompt picker");
