use pretty_assertions::assert_eq;
use serde_json::json;

use super::{InstallMethod, UpdateCheck, UpdateInfo};

// Covers: the `rho update --check --json` object rho-ui parses: field and
// method names, `update_available` from the version order, the command
// pinned to the latest release tag, and which methods need a terminal.
// Owner: update check report
#[test]
fn check_report_is_the_client_contract() {
    const LATEST_TAG: &str = "rho-coding-agent-v3.1.1";
    let update = |current: &str| UpdateInfo {
        latest_tag: LATEST_TAG.into(),
        latest_version: "3.1.1".into(),
        current_version: current.into(),
    };
    let script_command = InstallMethod::Script
        .update_command(LATEST_TAG)
        .expect("the script command needs no lookup");
    let cases = [
        (
            update("3.1.0"),
            InstallMethod::Pacman,
            json!({
                "current": "3.1.0",
                "latest": "3.1.1",
                "update_available": true,
                "method": "pacman",
                "command": "sudo pacman -Sy mjiang-extras/rho-coding-agent",
                "needs_terminal": true,
            }),
        ),
        (
            update("3.1.1"),
            InstallMethod::ScoopGlobal,
            json!({
                "current": "3.1.1",
                "latest": "3.1.1",
                "update_available": false,
                "method": "scoop-global",
                "command": "scoop update; scoop update -g rho",
                "needs_terminal": cfg!(windows),
            }),
        ),
        (
            update("3.1.0"),
            InstallMethod::Script,
            json!({
                "current": "3.1.0",
                "latest": "3.1.1",
                "update_available": true,
                "method": "script",
                "command": script_command,
                "needs_terminal": cfg!(windows),
            }),
        ),
    ];
    for (update, method, expected) in cases {
        let check = UpdateCheck::new(update, method).expect("command resolves without a lookup");
        assert_eq!(
            serde_json::to_value(&check).expect("the report serializes"),
            expected,
            "{method:?}"
        );
    }
}
