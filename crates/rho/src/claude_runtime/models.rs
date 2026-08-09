//! The Claude model vocabulary Rho offers when a surface picks one.
//!
//! Rho does not resolve Claude model names. `--model` is a pass-through, and
//! the `claude` binary has no command that enumerates models, so this list is
//! deliberately limited to family aliases: Claude resolves each one to the
//! latest model in that family, which keeps the list stable across model
//! launches. A full model id remains valid everywhere Rho accepts a model, it
//! is simply not offered as a row.

/// One offered Claude model alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeModelAlias {
    /// Value passed through as `--model`.
    pub(crate) name: &'static str,
    /// Row detail text, phrased for a picker.
    pub(crate) detail: &'static str,
}

/// Aliases offered in every Claude model picker, strongest family first.
pub(crate) const CLAUDE_MODEL_ALIASES: &[ClaudeModelAlias] = &[
    ClaudeModelAlias {
        name: "fable",
        detail: "Latest Claude Fable. Subscription plans differ in which families they include.",
    },
    ClaudeModelAlias {
        name: "opus",
        detail: "Latest Claude Opus. Subscription plans differ in which families they include.",
    },
    ClaudeModelAlias {
        name: "sonnet",
        detail: "Latest Claude Sonnet. Subscription plans differ in which families they include.",
    },
    ClaudeModelAlias {
        name: "haiku",
        detail: "Latest Claude Haiku. Subscription plans differ in which families they include.",
    },
];

/// Whether `model` is one of the offered aliases.
///
/// Used to decide whether a configured model already has a row, not to
/// validate it: Claude accepts full model ids that this list never carries.
pub(crate) fn is_offered_alias(model: &str) -> bool {
    CLAUDE_MODEL_ALIASES.iter().any(|alias| alias.name == model)
}

/// How Claude Code names itself where Rho would otherwise name a provider.
///
/// Display only: it fills the source slot in a `<source>/<model>` reference on
/// picker rows, status lines, and badges, and matches the `/login claude-code`
/// target a user types. It is not the persisted `runtime` key, which is
/// `crate::config::CLAUDE_CLI_RUNTIME_KEY` (`claude-cli`).
pub(crate) const CLAUDE_CODE_SOURCE_LABEL: &str = "claude-code";

/// Label for the row that omits `--model` and lets Claude Code choose.
pub(crate) const CLAUDE_DEFAULT_MODEL_LABEL: &str = "Claude Code default";

/// Detail for the default row.
pub(crate) const CLAUDE_DEFAULT_MODEL_DETAIL: &str =
    "Omit --model and use whichever model Claude Code selects.";

/// How a pinned model reads on a status line or badge when Claude Code picks
/// the model itself.
pub(crate) const CLAUDE_DEFAULT_MODEL_BADGE: &str = "default";
