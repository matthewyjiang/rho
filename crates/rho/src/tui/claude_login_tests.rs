use pretty_assertions::assert_eq;

use super::{
    resolve_claude_login_auth_outcome, CANCEL_LOGOUT_VALUE, CLAUDE_CODE_TARGET,
    CONFIRM_LOGOUT_VALUE, KEEP_LOGIN_VALUE, RELAY_LOGIN_VALUE,
};
use crate::claude_runtime::auth::{self, ClaudeAuthError, ClaudeAuthStatus};

#[test]
fn claude_code_target_is_stable() {
    assert_eq!(CLAUDE_CODE_TARGET, "claude-code");
    assert!(super::App::is_claude_code_target("claude-code"));
    assert!(super::App::is_claude_code_target(" Claude-Code "));
    assert!(!super::App::is_claude_code_target("anthropic"));
}

#[test]
fn handoff_and_logout_copy_keep_ownership_with_claude() {
    let handoff = auth::login_handoff_notice();
    assert!(handoff.contains("handing the terminal to the claude binary"));
    assert!(handoff.contains("Rho never sees or stores your token"));
    assert!(handoff.contains("/logout claude-code") || handoff.contains("claude auth logout"));

    let logout = auth::logout_confirm_description();
    assert!(logout.contains("everywhere the claude binary is used"));
    assert!(logout.contains("Rho does not store this credential"));
}

#[test]
fn relogin_and_logout_choice_values_are_stable() {
    assert_eq!(KEEP_LOGIN_VALUE, "keep");
    assert_eq!(RELAY_LOGIN_VALUE, "continue");
    assert_eq!(CONFIRM_LOGOUT_VALUE, "confirm");
    assert_eq!(CANCEL_LOGOUT_VALUE, "cancel");
}

fn signed_in_status() -> ClaudeAuthStatus {
    ClaudeAuthStatus {
        logged_in: true,
        auth_method: None,
        api_provider: None,
        email: Some("someone@example.com".into()),
        org_id: None,
        org_name: None,
        subscription_type: Some("pro".into()),
    }
}

fn signed_out_status() -> ClaudeAuthStatus {
    ClaudeAuthStatus {
        logged_in: false,
        auth_method: None,
        api_provider: None,
        email: None,
        org_id: None,
        org_name: None,
        subscription_type: None,
    }
}

#[tokio::test]
async fn successful_login_with_signed_in_status_is_complete() {
    let outcome =
        resolve_claude_login_auth_outcome(Ok(()), || async { Ok(signed_in_status()) }).await;
    assert_eq!(outcome.status_line(), "claude code login complete");
    assert!(outcome.summary_for_error().contains("someone@example.com"));
}

#[tokio::test]
async fn successful_login_with_signed_out_status_is_incomplete() {
    let outcome =
        resolve_claude_login_auth_outcome(Ok(()), || async { Ok(signed_out_status()) }).await;
    assert_eq!(outcome.status_line(), "claude code login incomplete");
    assert!(outcome
        .summary_for_error()
        .contains("status still reports signed out"));
}

#[tokio::test]
async fn successful_login_with_unreadable_status_is_incomplete() {
    let outcome =
        resolve_claude_login_auth_outcome(Ok(()), || async { Err(ClaudeAuthError::BinaryMissing) })
            .await;
    assert_eq!(outcome.status_line(), "claude code login incomplete");
    assert!(outcome
        .summary_for_error()
        .contains("status could not be read"));
}

#[tokio::test]
async fn failed_login_still_complete_when_status_shows_signed_in() {
    let outcome = resolve_claude_login_auth_outcome(
        Err(anyhow::anyhow!(
            "claude auth login exited with exit status: 1"
        )),
        || async { Ok(signed_in_status()) },
    )
    .await;
    assert_eq!(outcome.status_line(), "claude code login complete");
    assert!(outcome
        .summary_for_error()
        .contains("status shows signed in"));
}

#[tokio::test]
async fn failed_login_with_signed_out_status_is_failed() {
    let outcome = resolve_claude_login_auth_outcome(
        Err(anyhow::anyhow!(
            "claude auth login exited with exit status: 1"
        )),
        || async { Ok(signed_out_status()) },
    )
    .await;
    assert_eq!(outcome.status_line(), "claude code login failed");
    assert!(outcome.summary_for_error().contains("login failed"));
}

#[tokio::test]
async fn resume_failure_still_queries_and_keeps_restore_as_primary() {
    // Mirrors run_claude_code_login: auth outcome is always resolved, then the
    // resume error stays primary with the auth summary attached.
    let auth_outcome =
        resolve_claude_login_auth_outcome(Ok(()), || async { Ok(signed_in_status()) }).await;
    let resume_error = anyhow::anyhow!("failed to resume Rho after suspended operation")
        .context(auth_outcome.summary_for_error());
    let rendered = format!("{resume_error:#}");
    assert!(
        rendered.contains("failed to resume Rho after suspended operation"),
        "{rendered}"
    );
    assert!(rendered.contains("post-login auth status"), "{rendered}");
    assert!(rendered.contains("someone@example.com"), "{rendered}");
    assert_eq!(auth_outcome.status_line(), "claude code login complete");
}
