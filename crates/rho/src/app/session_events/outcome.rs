//! How a turn ended, as the daemon protocol tells clients (#1669): the
//! model's answer, or why the turn stopped or failed, and the provider's
//! diagnostic for a failure that may show one.

use rho_protocol::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};
use rho_sdk::{RunOutcome, StopReason};

use crate::{app::session_runtime::CommandRejected, config::Config};

/// The outcome of a turn whose run finished with `outcome`.
pub(crate) fn finished(outcome: &RunOutcome) -> TurnOutcome {
    let text = outcome.text().to_owned();
    let answered = |stop| TurnOutcome::Answered {
        text: text.clone(),
        stop,
    };
    match outcome.stop_reason() {
        StopReason::MaxSteps => TurnOutcome::MaxSteps { text: text.clone() },
        StopReason::EndTurn => answered(AnswerStop::EndTurn),
        StopReason::MaxTokens => answered(AnswerStop::MaxTokens),
        StopReason::Refusal => answered(AnswerStop::Refusal),
        // `StopReason` is non-exhaustive; a reason a newer SDK adds still
        // ends at the model's answer.
        _ => answered(AnswerStop::Unknown),
    }
}

/// Why a turn that failed with `error` failed, in a session whose model
/// runs as `model` selects: its kind, and the error with its causes.
pub(crate) fn turn_error(error: &anyhow::Error, model: &Config) -> TurnError {
    let kind = failure_kind(error);
    let message = match kind {
        // The provider's text may echo credentials, so an authentication
        // failure names only the selection and how to fix it.
        TurnErrorKind::Authentication => authentication_message(&model.provider, &model.auth),
        TurnErrorKind::Provider => provider_message(model, &chain_message(error)),
        _ => chain_message(error),
    };
    TurnError { kind, message }
}

/// `message` led by the provider that failed and the API base its requests
/// went to, the one `rho doctor` probes, so a failure such as a refused
/// connection says which host to check. Config loading refuses a base with
/// credentials or a query, so it shows whole.
fn provider_message(model: &Config, message: &str) -> String {
    let provider = &model.provider;
    match model.resolved_provider_endpoint(provider) {
        Some(base) => format!("{provider} at {base}: {message}"),
        None => format!("{provider}: {message}"),
    }
}

/// The notice that shows a failed turn's provider diagnostic: `None` when no
/// provider error in `error`'s chain has one, or when the turn failed to
/// authenticate, since the provider's details may echo credentials
/// ([`withheld_diagnostic`]).
pub(crate) fn diagnostic_notice(error: &anyhow::Error) -> Option<String> {
    if failure_kind(error) == TurnErrorKind::Authentication {
        return None;
    }
    let diagnostic = provider_diagnostic(error)?;
    Some(format!("provider diagnostic:\n{diagnostic}"))
}

/// The provider diagnostic [`diagnostic_notice`] keeps from clients: that of
/// a turn that failed to authenticate. For the host's own log only, where
/// the person running the daemon can read why the provider refused.
pub(crate) fn withheld_diagnostic(error: &anyhow::Error) -> Option<&str> {
    (failure_kind(error) == TurnErrorKind::Authentication)
        .then(|| provider_diagnostic(error))
        .flatten()
}

/// The diagnostic of the provider error in `error`'s chain, if it has one.
fn provider_diagnostic(error: &anyhow::Error) -> Option<&str> {
    error.chain().find_map(|cause| match cause.downcast_ref() {
        Some(rho_sdk::Error::Provider(provider)) => provider.diagnostic(),
        _ => None,
    })
}

/// `error` and its causes, each said once: an error whose message already
/// ends with its source's, as the SDK's do, does not repeat it.
fn chain_message(error: &anyhow::Error) -> String {
    let mut message = String::new();
    for cause in error.chain() {
        let text = cause.to_string();
        if message.ends_with(&text) {
            continue;
        }
        if !message.is_empty() {
            message.push_str(": ");
        }
        message.push_str(&text);
    }
    message
}

/// What kind of failure `error` is, from the typed errors in its chain,
/// never from its text.
fn failure_kind(error: &anyhow::Error) -> TurnErrorKind {
    for cause in error.chain() {
        // A prompt command the session cannot run, such as a TUI command
        // in a daemon session.
        if cause.downcast_ref::<CommandRejected>().is_some() {
            return TurnErrorKind::Configuration;
        }
        if let Some(error) = cause.downcast_ref::<rho_sdk::Error>() {
            return match error {
                rho_sdk::Error::Authentication { .. } => TurnErrorKind::Authentication,
                rho_sdk::Error::Provider(provider)
                    if provider.kind() == rho_sdk::ProviderErrorKind::Authentication =>
                {
                    TurnErrorKind::Authentication
                }
                rho_sdk::Error::Provider(_) => TurnErrorKind::Provider,
                rho_sdk::Error::Tool(_) => TurnErrorKind::ToolHost,
                rho_sdk::Error::InvalidConfiguration { .. } => TurnErrorKind::Configuration,
                _ => TurnErrorKind::Other,
            };
        }
        if let Some(error) = cause.downcast_ref::<rho_providers::model::ModelError>() {
            use rho_providers::model::ModelError;
            return match error {
                ModelError::MissingCredentials(_) | ModelError::Credentials(_) => {
                    TurnErrorKind::Authentication
                }
                ModelError::UnsupportedReasoning { .. } | ModelError::UnsupportedProvider(_) => {
                    TurnErrorKind::Configuration
                }
                _ => TurnErrorKind::Provider,
            };
        }
    }
    TurnErrorKind::Other
}

/// What an authentication failure says for `provider` with `auth`: only
/// those config identifiers and how to fix them, offering the provider's
/// other keyed auth modes as `--auth` fixes.
fn authentication_message(provider: &str, auth: &str) -> String {
    let alternatives = rho_providers::provider::provider_descriptor(provider)
        .into_iter()
        .flat_map(|descriptor| descriptor.auth_modes())
        .filter(|mode| {
            mode.id != auth
                && !matches!(
                    mode.auth_kind,
                    rho_providers::provider::ProviderAuthKind::None
                )
        })
        .map(|mode| format!("--auth {}", mode.id))
        .collect::<Vec<_>>();
    let mut message =
        format!("authentication failed for {provider} (auth: {auth}); run /login {provider}");
    if !alternatives.is_empty() {
        message.push_str(" or pass ");
        message.push_str(&alternatives.join(" or "));
    }
    message
}

#[cfg(test)]
#[path = "outcome_tests.rs"]
mod tests;
