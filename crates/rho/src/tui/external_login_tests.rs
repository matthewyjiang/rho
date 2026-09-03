use pretty_assertions::assert_eq;

use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    resolve_login_after_suspend, resolve_login_auth_outcome, LoginAfterSuspend, LoginAuthCopy,
    LoginAuthOutcome,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Auth {
    signed_in: bool,
}

#[derive(Clone, Copy, Debug)]
struct QueryErr(&'static str);

fn is_signed_in(status: &Auth) -> bool {
    status.signed_in
}

fn copy() -> LoginAuthCopy<Auth, QueryErr> {
    LoginAuthCopy {
        status_line_prefix: "test login",
        signed_in_notice: |_| "signed in".into(),
        incomplete_signed_out: |_| "still signed out".into(),
        incomplete_query_error: |error| format!("unread {}", error.0),
        failed: |error| format!("failed {error}"),
        child_failed_but_signed_in: |error, _| format!("signed in despite {error}"),
    }
}

// Covers: child result × auth probe maps onto complete / incomplete / failed
// Owner: tui external login
#[tokio::test]
async fn login_auth_outcome_covers_child_and_probe_cells() {
    let signed_in = Auth { signed_in: true };
    let signed_out = Auth { signed_in: false };
    let child_err = "login exited with 1";
    let cases = [
        (
            Ok(()),
            Ok(signed_in),
            LoginAuthOutcome::Complete {
                notice: "signed in".into(),
            },
        ),
        (
            Ok(()),
            Ok(signed_out),
            LoginAuthOutcome::Incomplete {
                message: "still signed out".into(),
            },
        ),
        (
            Ok(()),
            Err(QueryErr("probe")),
            LoginAuthOutcome::Incomplete {
                message: "unread probe".into(),
            },
        ),
        (
            Err(child_err),
            Ok(signed_in),
            LoginAuthOutcome::Complete {
                notice: format!("signed in despite {child_err}"),
            },
        ),
        (
            Err(child_err),
            Ok(signed_out),
            LoginAuthOutcome::Failed {
                message: format!("failed {child_err}"),
            },
        ),
        (
            Err(child_err),
            Err(QueryErr("probe")),
            LoginAuthOutcome::Failed {
                message: format!("failed {child_err}"),
            },
        ),
    ];

    for (operation, query, expected) in cases {
        let operation = operation.map_err(anyhow::Error::msg);
        let (outcome, _) =
            resolve_login_auth_outcome(operation, || async { query }, is_signed_in, &copy()).await;
        assert_eq!(outcome, expected);
    }
}

#[tokio::test]
async fn resume_failure_does_not_query_auth_status() {
    let query_calls = AtomicUsize::new(0);
    let result = resolve_login_after_suspend(
        Err(anyhow::anyhow!(
            "failed to resume Rho after suspended operation"
        )),
        Ok(()),
        || {
            query_calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(Auth { signed_in: true }) }
        },
        is_signed_in,
        &copy(),
        "test login",
    )
    .await;

    assert_eq!(query_calls.load(Ordering::SeqCst), 0);
    match result {
        LoginAfterSuspend::ResumeFailed { error } => {
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("failed to resume Rho after suspended operation"),
                "{rendered}"
            );
        }
        LoginAfterSuspend::AuthResolved { outcome, .. } => {
            panic!("expected resume failure, got auth outcome {outcome:?}");
        }
    }
}

#[tokio::test]
async fn resume_failure_keeps_restore_primary_and_attaches_child_error() {
    let query_calls = AtomicUsize::new(0);
    let result = resolve_login_after_suspend(
        Err(anyhow::anyhow!(
            "failed to resume Rho after suspended operation"
        )),
        Err(anyhow::anyhow!("login exited with exit status: 1")),
        || {
            query_calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(Auth { signed_in: false }) }
        },
        is_signed_in,
        &copy(),
        "test login",
    )
    .await;

    assert_eq!(query_calls.load(Ordering::SeqCst), 0);
    match result {
        LoginAfterSuspend::ResumeFailed { error } => {
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("failed to resume Rho after suspended operation"),
                "{rendered}"
            );
            assert!(rendered.contains("test login also failed"), "{rendered}");
            assert!(
                rendered.contains("login exited with exit status: 1"),
                "{rendered}"
            );
        }
        LoginAfterSuspend::AuthResolved { outcome, .. } => {
            panic!("expected resume failure, got auth outcome {outcome:?}");
        }
    }
}

#[tokio::test]
async fn successful_resume_queries_auth_after_child_success() {
    let query_calls = AtomicUsize::new(0);
    let result = resolve_login_after_suspend(
        Ok(()),
        Ok(()),
        || {
            query_calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(Auth { signed_in: true }) }
        },
        is_signed_in,
        &copy(),
        "test login",
    )
    .await;

    assert_eq!(query_calls.load(Ordering::SeqCst), 1);
    match result {
        LoginAfterSuspend::AuthResolved { outcome, .. } => {
            assert_eq!(
                outcome,
                LoginAuthOutcome::Complete {
                    notice: "signed in".into(),
                }
            );
        }
        LoginAfterSuspend::ResumeFailed { error } => {
            panic!("expected auth outcome, got resume failure {error:#}");
        }
    }
}
