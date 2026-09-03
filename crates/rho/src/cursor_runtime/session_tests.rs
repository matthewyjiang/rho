use crate::agent::PromptPolicy;

use super::*;

fn system_prompt() -> PromptPolicy {
    PromptPolicy::Extend(String::new())
}

fn logged_in() -> CursorAuthStatus {
    CursorAuthStatus {
        status: "authenticated".into(),
        is_authenticated: true,
        message: None,
        user_info: Some(super::auth::CursorUserInfo {
            email: Some("t@example.com".into()),
        }),
    }
}

fn cursor_identity() -> RunArtifactIdentity {
    RunArtifactIdentity {
        agent_id: "cursor-worker".into(),
        agent_fingerprint: "fp".into(),
        provider: "cursor".into(),
        model: Some("composer-2.5".into()),
        runtime: crate::agent::AgentRuntime::Cursor,
        reasoning: None,
    }
}

#[cfg(unix)]
#[path = "session_process_tests.rs"]
mod unix_fake_matrix;
