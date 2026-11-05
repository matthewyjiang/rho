//! The credential store question `authenticate` asks before a login when
//! none is chosen, as `rho login` asks it in a terminal. The answer is not a
//! secret, so it goes to the host as a form scoped to the `authenticate`
//! request, and Rho saves it to config the way the terminal login does.

use std::path::PathBuf;

use agent_client_protocol::{
    schema::v1::{ElicitationRequestScope, RequestId},
    Error as AcpError,
};
use rho_providers::credentials::CredentialStoreBackend;
use rho_sdk::{HostChoice, HostInputRequest, HostQuestion, SelectionMode};

use super::{elicitation, host_features::Forms, AcpClientPort};
use crate::{
    app::{bootstrap, config_repository::ConfigRepository},
    credential_store::{self, StoreChoiceRequest},
};

const QUESTION: &str = "store";

/// Asks where to store credentials and saves the answer to the config at
/// the `--config` override `config`, unless a store is already chosen.
pub(super) async fn choose_if_unset(
    config: Option<PathBuf>,
    forms: Forms,
    request_id: &RequestId,
    port: &dyn AcpClientPort,
) -> Result<(), AcpError> {
    let repository = ConfigRepository::new(config);
    let loaded = repository.load().map_err(internal)?;
    let Some(choices) = credential_store::choice_request(&loaded) else {
        return Ok(());
    };
    let backend = ask(&choices, forms, request_id, port).await?;
    let path = bootstrap::absolute_config_path(&repository).map_err(internal)?;
    credential_store::set_backend(backend, Some(path)).map_err(|error| {
        AcpError::internal_error().data(format!(
            "could not save the credential store choice: {error:#}"
        ))
    })?;
    Ok(())
}

/// Puts `choices` to the user as a form and returns the store they chose.
async fn ask(
    choices: &StoreChoiceRequest,
    forms: Forms,
    request_id: &RequestId,
    port: &dyn AcpClientPort,
) -> Result<CredentialStoreBackend, AcpError> {
    let dialect = match forms {
        Forms::Available(dialect) => dialect,
        // `initialize` advertised terminal logins instead; the store was
        // unset then too, or was unset since.
        Forms::Unavailable => {
            return Err(AcpError::internal_error().data(
                "credential store is unset; choose one with `rho credential-store set os|file`",
            ))
        }
    };
    let question = question(choices)?;
    let form = elicitation::form(
        &question,
        ElicitationRequestScope::new(request_id.clone()),
        dialect,
    );
    let response = port.create_elicitation(form).await.map_err(|error| {
        AcpError::internal_error().data(format!(
            "could not ask where to store credentials: {error}; run `rho credential-store set os|file` in a terminal instead"
        ))
    })?;
    // Declining or dismissing the question abandons the sign-in, as it does
    // for the sign-in link.
    let answers = elicitation::host_answer(&question, response, dialect)
        .map_err(|_| AcpError::request_cancelled())?;
    let chosen = answers
        .answers()
        .get(QUESTION)
        .and_then(|values| values.first())
        .and_then(|value| CredentialStoreBackend::parse(value).ok());
    chosen.ok_or_else(AcpError::request_cancelled)
}

/// The question, offering only the stores that can be opened here.
fn question(choices: &StoreChoiceRequest) -> Result<HostInputRequest, AcpError> {
    let options = choices
        .available_backends()
        .into_iter()
        .map(|backend| match backend {
            CredentialStoreBackend::Os => {
                HostChoice::new("os", "OS credential store (recommended)")
            }
            CredentialStoreBackend::File => HostChoice::new(
                "file",
                "Local file under ~/.rho/credentials (not encrypted at rest)",
            ),
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        return Err(AcpError::internal_error().data(format!(
            "no credential store backend is available (os: {}; file: {})",
            choices.os.detail, choices.file.detail
        )));
    }
    let mut question = HostQuestion::new(
        QUESTION,
        "Where should Rho store provider credentials?",
        options,
        SelectionMode::One,
    )
    .map_err(internal)?
    .with_help("Rho saves this choice to config and uses it for future logins on this machine.");
    if let Some(default) = choices.default_backend() {
        question = question.with_default_value(serde_json::Value::from(default.as_str()));
    }
    HostInputRequest::questionnaire("Choose where Rho stores credentials", vec![question])
        .map_err(internal)
}

fn internal(error: impl std::fmt::Display) -> AcpError {
    AcpError::internal_error().data(format!("{error:#}"))
}

#[cfg(test)]
#[path = "auth_store_tests.rs"]
mod tests;
