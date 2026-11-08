//! How a turn ended, as the daemon protocol tells clients (#1669): the
//! model's answer, or why the turn stopped or failed.

use rho_protocol::{AnswerStop, TurnError, TurnErrorKind, TurnOutcome};
use rho_sdk::{RunOutcome, StopReason};

use crate::app::session_runtime::CommandRejected;

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
/// runs on `provider` with `auth`: its kind, and the error with its causes.
pub(crate) fn turn_error(error: &anyhow::Error, provider: &str, auth: &str) -> TurnError {
    let kind = failure_kind(error);
    // The provider's text may echo credentials, so an authentication
    // failure names only the selection and how to fix it.
    let message = if kind == TurnErrorKind::Authentication {
        authentication_message(provider, auth)
    } else {
        chain_message(error)
    };
    TurnError { kind, message }
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
