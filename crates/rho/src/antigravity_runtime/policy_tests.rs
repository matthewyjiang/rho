use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::agent::AntigravityTool as T;
use crate::antigravity_runtime::home::TokenStore;

fn signed_in_home(root: &Path) -> AntigravityHome {
    let home = AntigravityHome::resolve(Some(root.into()), Path::new("/unused"), TokenStore::File);
    std::fs::create_dir_all(home.acp_dir()).unwrap();
    std::fs::write(
        home.settings_path(),
        r#"{"auth":{"type":"oauth-personal"}}"#,
    )
    .unwrap();
    std::fs::write(home.acp_dir().join("acp_token.json"), "{}").unwrap();
    home
}

fn policy(
    mode: PermissionMode,
    model: Option<&str>,
    home: AntigravityHome,
) -> AntigravityAcpPolicy {
    AntigravityAcpPolicy::new(
        model.map(str::to_owned),
        mode,
        vec![T::ViewFile, T::RunCommand],
        PathBuf::from("/work"),
        home,
        vec![],
    )
}

// Covers: a prepared run pins mode `default` (never a configured `yolo`),
// allowlists the mode-narrowed built-ins, sets a pinned model as the
// session option, and never authenticates.
// Owner: Antigravity policy wiring; the driver applying these is covered in
// `acp_runtime` session tests, the narrowing itself in `fence_tests`.
#[test]
fn prepared_run_pins_mode_tools_and_model() {
    let root = tempfile::tempdir().unwrap();
    let mut policy = policy(
        PermissionMode::Plan,
        Some("gemini-3.8-flash-high"),
        signed_in_home(root.path()),
    );
    assert_eq!(policy.session_meta(), None, "no allowlist before prepare");
    assert_eq!(policy.prepare(root.path()), Ok(vec![]));
    assert_eq!(
        (
            policy.session_mode(),
            policy.session_meta().map(serde_json::Value::Object),
            policy.session_config(),
            policy.auth_method(&[]),
        ),
        (
            Some(SessionModeId::new("default")),
            Some(json!({"agy": {"enabledTools": ["view_file"]}})),
            vec![SessionConfigChoice {
                id: "model".into(),
                value: "gemini-3.8-flash-high".into(),
            }],
            None,
        )
    );
}

// Covers: a run fails before spawning when signed out (it would otherwise
// hang in the server's browser flow) or in a mode Rho cannot answer for.
// Owner: Antigravity preflight; message wording is not locked.
#[test]
fn prepare_refuses_unanswerable_runs() {
    let signed_out = tempfile::tempdir().unwrap();
    let signed_in = tempfile::tempdir().unwrap();
    for (mode, home, hint) in [
        (
            PermissionMode::Bypass,
            AntigravityHome::resolve(
                Some(signed_out.path().into()),
                Path::new("/unused"),
                TokenStore::File,
            ),
            "login antigravity",
        ),
        (
            PermissionMode::Auto,
            signed_in_home(signed_in.path()),
            "Plan or Bypass",
        ),
    ] {
        let error = policy(mode, None, home)
            .prepare(signed_in.path())
            .unwrap_err();
        assert!(error.contains(hint), "{mode}: {error}");
    }
}
