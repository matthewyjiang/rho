use serde_json::json;

use super::*;

fn context(cwd: std::path::PathBuf, max_output_bytes: usize) -> ToolContext {
    ToolContext {
        cwd,
        max_output_bytes,
    }
}

#[tokio::test]
async fn captures_large_final_output_burst() {
    let result = PowerShell::new(false)
        .call(
            json!({"command": "[Console]::Out.Write('x' * 100000)"}),
            context(std::env::temp_dir(), 200_000),
            "call_1".into(),
        )
        .await
        .unwrap();

    let stdout = result
        .content
        .strip_prefix("stdout:\n")
        .unwrap()
        .split_once("\n\nstderr:")
        .unwrap()
        .0;
    assert_eq!(stdout.len(), 100_000);
}

#[tokio::test]
async fn timeout_terminates_background_processes() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("background-process-survived");
    let escaped_marker = marker.display().to_string().replace('\'', "''");
    let command = format!(
        "Start-Process powershell.exe -ArgumentList '-NoProfile','-Command',\"Start-Sleep 2; New-Item -ItemType File -Path '{escaped_marker}'\"; Start-Sleep 10"
    );

    PowerShell::new(false)
        .call(
            json!({"command": command, "timeout_seconds": 1}),
            context(dir.path().to_path_buf(), 12_000),
            "call_1".into(),
        )
        .await
        .unwrap_err();

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert!(!marker.exists(), "background process survived the timeout");
}
