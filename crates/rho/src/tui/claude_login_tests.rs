use pretty_assertions::assert_eq;

use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    resolve_claude_login_after_suspend, resolve_claude_login_auth_outcome, ClaudeLoginAfterSuspend,
    ClaudeLoginAuthOutcome, SignInTarget,
};
use crate::claude_runtime::auth::ClaudeAuthStatus;

#[test]
fn sign_in_target_routes_claude_code_case_insensitively() {
    for value in ["claude-code", " Claude-Code "] {
        assert!(
            matches!(SignInTarget::parse(value), SignInTarget::ClaudeCode),
            "{value} must route to the external runtime"
        );
    }
    assert!(matches!(
        SignInTarget::parse(" anthropic "),
        SignInTarget::Provider(provider) if provider == "anthropic"
    ));
}

// Covers: /login picker values must map each custom host API
// Owner: login routing
#[test]
fn sign_in_target_routes_custom_host_api_methods() {
    assert!(matches!(
        SignInTarget::parse(
            super::super::custom_provider_login::NEW_CUSTOM_CHAT_COMPLETIONS_HOST_VALUE
        ),
        SignInTarget::NewCustomHost {
            api: rho_providers::provider::OpenAiCompatibleApi::ChatCompletions
        }
    ));
    assert!(matches!(
        SignInTarget::parse(super::super::custom_provider_login::NEW_CUSTOM_RESPONSES_HOST_VALUE),
        SignInTarget::NewCustomHost {
            api: rho_providers::provider::OpenAiCompatibleApi::Responses
        }
    ));
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
    match outcome {
        ClaudeLoginAuthOutcome::Complete { notice } => {
            assert!(notice.contains("someone@example.com"));
        }
        other => panic!("expected complete outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn successful_login_with_signed_out_status_is_incomplete() {
    let outcome =
        resolve_claude_login_auth_outcome(Ok(()), || async { Ok(signed_out_status()) }).await;
    assert_eq!(outcome.status_line(), "claude code login incomplete");
    match outcome {
        ClaudeLoginAuthOutcome::Incomplete { message } => {
            assert!(message.contains("status still reports signed out"));
        }
        other => panic!("expected incomplete outcome, got {other:?}"),
    }
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
    match outcome {
        ClaudeLoginAuthOutcome::Complete { notice } => {
            assert!(notice.contains("status shows signed in"));
        }
        other => panic!("expected complete outcome, got {other:?}"),
    }
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
    match outcome {
        ClaudeLoginAuthOutcome::Failed { message } => {
            assert!(message.contains("login failed"));
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_failure_does_not_query_auth_status() {
    let query_calls = AtomicUsize::new(0);
    let result = resolve_claude_login_after_suspend(
        Err(anyhow::anyhow!(
            "failed to resume Rho after suspended operation"
        )),
        Ok(()),
        || {
            query_calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(signed_in_status()) }
        },
    )
    .await;

    assert_eq!(query_calls.load(Ordering::SeqCst), 0);
    match result {
        ClaudeLoginAfterSuspend::ResumeFailed { error } => {
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("failed to resume Rho after suspended operation"),
                "{rendered}"
            );
            assert!(!rendered.contains("post-login auth status"), "{rendered}");
        }
        ClaudeLoginAfterSuspend::AuthResolved { outcome } => {
            panic!("expected resume failure, got auth outcome {outcome:?}");
        }
    }
}

#[tokio::test]
async fn resume_failure_keeps_restore_primary_and_attaches_child_error() {
    let query_calls = AtomicUsize::new(0);
    let result = resolve_claude_login_after_suspend(
        Err(anyhow::anyhow!(
            "failed to resume Rho after suspended operation"
        )),
        Err(anyhow::anyhow!(
            "claude auth login exited with exit status: 1"
        )),
        || {
            query_calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(signed_out_status()) }
        },
    )
    .await;

    assert_eq!(query_calls.load(Ordering::SeqCst), 0);
    match result {
        ClaudeLoginAfterSuspend::ResumeFailed { error } => {
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("failed to resume Rho after suspended operation"),
                "{rendered}"
            );
            assert!(
                rendered.contains("claude auth login also failed"),
                "{rendered}"
            );
            assert!(
                rendered.contains("claude auth login exited with exit status: 1"),
                "{rendered}"
            );
        }
        ClaudeLoginAfterSuspend::AuthResolved { outcome } => {
            panic!("expected resume failure, got auth outcome {outcome:?}");
        }
    }
}

#[tokio::test]
async fn successful_resume_queries_auth_after_child_success() {
    let query_calls = AtomicUsize::new(0);
    let result = resolve_claude_login_after_suspend(Ok(()), Ok(()), || {
        query_calls.fetch_add(1, Ordering::SeqCst);
        async { Ok(signed_in_status()) }
    })
    .await;

    assert_eq!(query_calls.load(Ordering::SeqCst), 1);
    match result {
        ClaudeLoginAfterSuspend::AuthResolved { outcome } => {
            assert_eq!(outcome.status_line(), "claude code login complete");
        }
        ClaudeLoginAfterSuspend::ResumeFailed { error } => {
            panic!("expected auth outcome, got resume failure {error:#}");
        }
    }
}
