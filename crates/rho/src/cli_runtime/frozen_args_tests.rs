use pretty_assertions::assert_eq;

use super::overlay_identity_flags;

// Covers: overlay replaces an existing identity flag, appends a missing one,
// and never copies flags outside the caller's identity list.
// Owner: cli frozen argv overlay
#[test]
fn overlay_replaces_appends_and_ignores_non_identity_flags() {
    let generated = vec![
        "-p".into(),
        "--model".into(),
        "generated".into(),
        "--mode".into(),
        "plan".into(),
    ];
    let frozen = vec![
        "--model".into(),
        "frozen-model".into(),
        "--effort".into(),
        "high".into(),
        "--mode".into(),
        "full".into(),
        "--allowed-tools".into(),
        "shell_tool_call".into(),
    ];
    let args = overlay_identity_flags(generated, &frozen, &["--model", "--effort"]);
    assert_eq!(
        args,
        vec![
            "-p".to_string(),
            "--model".into(),
            "frozen-model".into(),
            "--mode".into(),
            "plan".into(),
            "--effort".into(),
            "high".into(),
        ]
    );
}
