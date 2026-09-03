//! `/login` and `/logout` argument routing for external runtimes and hosts.

use rho_providers::provider::OpenAiCompatibleApi;

use crate::agent::AgentRuntime;

use super::claude_login::CLAUDE_CODE_TARGET;

const CURSOR_AGENT_ALIAS: &str = "cursor-agent";

/// Login methods that are not Rho provider credentials.
///
/// The picker renders whatever it is handed; `group_id: None` is a top-level
/// row, while `Some(id)` nests the method under that login group.
pub(super) fn external_login_methods() -> [ExternalLoginMethod; 2] {
    [
        ExternalLoginMethod {
            // Claude Code is an Anthropic-family runtime for delegation, not a
            // separate top-level provider group.
            group_id: Some("anthropic"),
            value: CLAUDE_CODE_TARGET,
            label: "Claude Code (delegation only)",
            detail: "External Claude binary subscription, not Anthropic API billing. \
Credentials are managed by Claude Code, not Rho.",
        },
        ExternalLoginMethod {
            group_id: None,
            value: AgentRuntime::Cursor.as_str(),
            label: "Cursor",
            detail: "Cursor Agent CLI (cursor-agent login)",
        },
    ]
}

/// One login method backed by an external runtime rather than a Rho credential.
pub(super) struct ExternalLoginMethod {
    /// Login group this method is offered under. `None` lists it at the top level.
    pub(super) group_id: Option<&'static str>,
    /// Picker value, which is also the `/login` argument.
    pub(super) value: &'static str,
    pub(super) label: &'static str,
    pub(super) detail: &'static str,
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
