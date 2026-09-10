//! CLI/editor process coverage. Filesystem transaction rules live beside the editor store.
#![cfg(unix)]

use pretty_assertions::assert_eq;
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_rho"));
        command
            .args(["model-prompt", "edit"])
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
            vec![],
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
        let output = fixture.command().output().unwrap();
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
            command.env_remove("VISUAL").env_remove("EDITOR");
        } else {
            command.args(["--provider", "openai", "--model", "@local"]);
        }
        assert!(!command.output().unwrap().status.success());
        assert!(!fixture.root.path().join("draft-path").exists());
        assert!(!fixture.directory().exists());
    }
}
