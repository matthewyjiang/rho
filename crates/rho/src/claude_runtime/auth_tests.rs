use pretty_assertions::assert_eq;
use tokio::process::Command;

use super::*;
use crate::cli_runtime::ProbeError;

/// Stable system shell used by process tests.
///
/// Tests never exec a freshly written file: under parallel load Linux can
/// return `ETXTBSY` when a just-created script is still open for write
/// elsewhere. Script bodies live in argv (`/bin/sh -c <body> -- <args…>`).
#[cfg(unix)]
const STABLE_SH: &str = "/bin/sh";

/// Build a probe command that runs `body` under `/bin/sh -c` and appends the
/// auth argv after `--` so the script sees them as `$1`, `$2`, …
#[cfg(unix)]
fn sh_probe_command(body: &str, args: &[&str]) -> Command {
    let mut command = Command::new(STABLE_SH);
    command.arg("-c").arg(body).arg("--");
    command.args(args);
    command
}

#[cfg(unix)]
async fn run_sh_probe(body: &str, args: &[&str]) -> Result<BoundedOutput, ClaudeAuthError> {
    run_bounded_command_with_timeout(
        sh_probe_command(body, args),
        STABLE_SH.into(),
        PROBE_TIMEOUT,
    )
    .await
    .map_err(ClaudeAuthError::from)
}

#[cfg(unix)]
async fn query_sh(body: &str) -> Result<ClaudeAuthStatus, ClaudeAuthError> {
    let output = run_sh_probe(body, &["auth", "status"]).await?;
    parse_auth_status_output(STABLE_SH, &output)
}

#[cfg(unix)]
async fn logout_sh(body: &str) -> Result<(), ClaudeAuthError> {
    let output = run_sh_probe(body, &["auth", "logout"]).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ClaudeAuthError::ExitStatus {
            program: STABLE_SH.into(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        })
    }
}

#[cfg(unix)]
async fn version_sh(body: &str) -> Result<String, ClaudeAuthError> {
    let output = run_sh_probe(body, &["--version"]).await?;
    if !output.status.success() {
        return Err(ClaudeAuthError::ExitStatus {
            program: STABLE_SH.into(),
            status: output.status,
            stderr: output.stderr_lossy_trimmed(),
        });
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| ClaudeAuthError::InvalidUtf8)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let version = first_nonempty_line(&stdout)
        .or_else(|| first_nonempty_line(&stderr))
        .unwrap_or("unknown version")
        .to_string();
    Ok(version)
}

#[test]
fn parses_optional_auth_fields_and_requires_logged_in() {
    let status: ClaudeAuthStatus = serde_json::from_str(
        r#"{
            "loggedIn": true,
            "email": "someone@example.com"
        }"#,
    )
    .unwrap();
    assert!(status.logged_in);
    assert_eq!(status.email.as_deref(), Some("someone@example.com"));
    assert_eq!(status.subscription_type, None);

    let error = serde_json::from_str::<ClaudeAuthStatus>(r#"{"email":"x"}"#).unwrap_err();
    assert!(error.to_string().contains("loggedIn") || error.to_string().contains("missing field"));
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_parses_signed_in_status_json() {
    let status = query_sh(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' '{"loggedIn":true,"email":"someone@example.com","subscriptionType":"pro"}'
  exit 0
fi
echo "unexpected: $*" >&2
exit 1
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        status,
        ClaudeAuthStatus {
            logged_in: true,
            auth_method: None,
            api_provider: None,
            email: Some("someone@example.com".into()),
            org_id: None,
            org_name: None,
            subscription_type: Some("pro".into()),
        }
    );
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_parses_signed_out_status_json() {
    let status = query_sh(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' '{"loggedIn":false}'
  exit 0
fi
exit 1
"#,
    )
    .await
    .unwrap();
    assert!(!status.logged_in);
    assert_eq!(status.email, None);
    assert_eq!(status.subscription_type, None);
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_accepts_valid_signed_out_json_on_nonzero_exit() {
    // Real `claude auth status` returns exit 1 with this shape when signed out.
    let status = query_sh(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}'
  exit 1
fi
exit 2
"#,
    )
    .await
    .unwrap();
    assert_eq!(
        status,
        ClaudeAuthStatus {
            logged_in: false,
            auth_method: Some("none".into()),
            api_provider: Some("firstParty".into()),
            email: None,
            org_id: None,
            org_name: None,
            subscription_type: None,
        }
    );
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_rejects_malformed_json_on_nonzero_exit() {
    let error = query_sh(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' 'not-json'
  echo boom >&2
  exit 1
fi
exit 2
"#,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, ClaudeAuthError::InvalidJson(_)),
        "malformed stdout must stay InvalidJson, not ExitStatus: {error:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_reports_exit_status_when_stdout_empty_on_nonzero_exit() {
    let error = query_sh(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  echo boom >&2
  exit 1
fi
exit 2
"#,
    )
    .await
    .unwrap_err();
    match error {
        ClaudeAuthError::ExitStatus { stderr, .. } => assert_eq!(stderr, "boom"),
        other => panic!("expected ExitStatus, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_rejects_oversized_stdout() {
    let oversized = PROBE_OUTPUT_CAP_BYTES + 1;
    let body = format!(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  # Emit more than the probe cap without sleeping.
  head -c {oversized} /dev/zero
  exit 0
fi
exit 1
"#
    );
    let error = query_sh(&body).await.unwrap_err();
    match error {
        ClaudeAuthError::Probe(ProbeError::OutputTooLarge { stream, cap, .. }) => {
            assert_eq!(stream, "stdout");
            assert_eq!(cap, PROBE_OUTPUT_CAP_BYTES);
        }
        other => panic!("expected OutputTooLarge, got {other:?}"),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_reports_missing_binary() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-claude");
    // Explicit missing-path spawn: resolve_named must not invent a binary.
    let error = executable::resolve_named(missing.to_str().unwrap()).unwrap_err();
    assert!(error.is_binary_missing());

    // Also cover the probe spawn path when the program image is absent.
    let mut command = Command::new(&missing);
    command.args(["auth", "status"]);
    let spawn_error =
        run_bounded_command_with_timeout(command, missing.display().to_string(), PROBE_TIMEOUT)
            .await
            .unwrap_err();
    assert!(spawn_error.is_binary_missing());
}

#[cfg(unix)]
#[tokio::test]
async fn query_program_rejects_malformed_json() {
    let error = query_sh(
        r#"
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' 'not-json'
  exit 0
fi
exit 1
"#,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ClaudeAuthError::InvalidJson(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn version_program_reads_first_line() {
    let version = version_sh(
        r#"
if [ "$1" = "--version" ]; then
  printf '%s\n' '1.2.3 (Claude Code)'
  exit 0
fi
exit 1
"#,
    )
    .await
    .unwrap();
    assert_eq!(version, "1.2.3 (Claude Code)");
}

#[cfg(unix)]
#[tokio::test]
async fn logout_then_status_is_source_of_truth_even_on_nonzero_exit() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("state");
    std::fs::write(&state, "in").unwrap();
    let body = format!(
        r#"
STATE="{state}"
if [ "$1" = "auth" ] && [ "$2" = "logout" ]; then
  printf 'out' > "$STATE"
  echo boom >&2
  exit 7
fi
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  if [ "$(cat "$STATE")" = "out" ]; then
    printf '%s\n' '{{"loggedIn":false}}'
  else
    printf '%s\n' '{{"loggedIn":true,"email":"x@y.z"}}'
  fi
  exit 0
fi
exit 1
"#,
        state = state.display()
    );

    let logout_error = logout_sh(&body).await.unwrap_err();
    assert!(matches!(logout_error, ClaudeAuthError::ExitStatus { .. }));
    assert_eq!(logout_error.stderr_excerpt(), Some("boom"));

    let status = query_sh(&body).await.unwrap();
    assert!(
        !status.logged_in,
        "status after failed logout child must still report signed out"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn logout_already_signed_out_status_is_success_truth() {
    let body = r#"
if [ "$1" = "auth" ] && [ "$2" = "logout" ]; then
  echo "not logged in" >&2
  exit 1
fi
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '%s\n' '{"loggedIn":false}'
  exit 0
fi
exit 1
"#;

    let _ = logout_sh(body).await;
    let status = query_sh(body).await.unwrap();
    assert!(!status.logged_in);
}
