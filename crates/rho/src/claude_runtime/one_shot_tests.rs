use super::*;

fn exit_status() -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        std::process::Command::new("false").status().unwrap()
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "exit 1"])
            .status()
            .unwrap()
    }
}

// Covers: advisor callers receive the shared unsupported-flag diagnosis, not
// a generic missing-result error.
// Owner: Claude one-shot adapter.
#[test]
fn unsupported_max_turns_uses_shared_terminal_diagnosis() {
    let result = finish(
        String::new(),
        None,
        "error: unknown option '--max-turns'",
        exit_status(),
    );
    let Err(error) = result else {
        panic!("unsupported --max-turns must fail");
    };

    assert_eq!(
        error,
        "claude code: this claude binary rejected --max-turns; upgrade Claude Code or remove the turn cap"
    );
}
