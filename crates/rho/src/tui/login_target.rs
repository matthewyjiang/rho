//! `/login` and `/logout` argument routing for external runtimes and hosts.

use rho_providers::provider::OpenAiCompatibleApi;

use crate::agent::AgentRuntime;

use super::claude_login::CLAUDE_CODE_TARGET;

const CURSOR_AGENT_ALIAS: &str = "cursor-agent";

/// Delegated runtime sign-ins, which are not Rho provider credentials.
///
/// Each one nests under its vendor's login group rather than standing as a
/// top-level provider, since none of them can serve a Rho conversation.
pub(super) fn external_login_methods() -> [ExternalLoginMethod; 3] {
    [
        ExternalLoginMethod {
            group_id: "anthropic",
            value: CLAUDE_CODE_TARGET,
            name: "Claude Code",
            detail: "Uses the claude CLI, not Anthropic API billing. \
Claude Code manages credentials, not Rho.",
        },
        ExternalLoginMethod {
            group_id: "google",
            value: AgentRuntime::Antigravity.as_str(),
            name: "Antigravity",
            detail: "Uses agy_acp_server, not Gemini API billing. \
Antigravity manages credentials, not Rho.",
        },
        ExternalLoginMethod {
            group_id: "xai",
            value: AgentRuntime::Cursor.as_str(),
            name: "Cursor",
            detail: "Uses the cursor-agent CLI, not SpaceXAI API billing. \
Cursor manages credentials, not Rho.",
        },
    ]
}

/// One login method backed by a delegated runtime rather than a Rho credential.
pub(super) struct ExternalLoginMethod {
    /// Login group this method is offered under.
    pub(super) group_id: &'static str,
    /// Picker value, which is also the `/login` argument.
    pub(super) value: &'static str,
    /// Runtime name; the picker label adds the shared delegation marker.
    pub(super) name: &'static str,
    pub(super) detail: &'static str,
}

impl ExternalLoginMethod {
    /// Picker label, marked the same way for every delegated runtime.
    pub(super) fn label(&self) -> String {
        format!("{} (delegation only)", self.name)
    }
}

/// What a `/login` or `/logout` argument names.
///
/// Parsed once at each command or picker boundary so the provider flows never
/// re-sniff for an external runtime.
pub(super) enum SignInTarget {
    /// Claude Code, whose credential the `claude` binary owns.
    ClaudeCode,
    /// Cursor Agent CLI, whose credential `cursor-agent` owns.
    Cursor,
    /// Antigravity ACP server, which owns its Google sign-in.
    Antigravity,
    /// Onboarding for a host that does not exist yet.
    NewCustomHost { api: OpenAiCompatibleApi },
    /// A Rho provider credential.
    Provider(String),
}

impl SignInTarget {
    pub(super) fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.eq_ignore_ascii_case(CLAUDE_CODE_TARGET) {
            Self::ClaudeCode
        } else if is_cursor_login_target(value) {
            Self::Cursor
        } else if value.eq_ignore_ascii_case(AgentRuntime::Antigravity.as_str()) {
            Self::Antigravity
        } else if let Some(api) = super::custom_provider_login::parse_custom_host_api(value) {
            Self::NewCustomHost { api }
        } else {
            Self::Provider(value.to_string())
        }
    }
}

fn is_cursor_login_target(value: &str) -> bool {
    value.eq_ignore_ascii_case(AgentRuntime::Cursor.as_str())
        || value.eq_ignore_ascii_case(CURSOR_AGENT_ALIAS)
}

#[cfg(test)]
#[path = "login_target_tests.rs"]
mod tests;
