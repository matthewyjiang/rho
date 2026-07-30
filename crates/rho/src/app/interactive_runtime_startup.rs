//! Resolving what an interactive session starts from.
//!
//! Provider construction, resumed-snapshot selection, prompt cache keys, and the
//! approval channel are all "how do we begin" decisions. Keeping them together
//! leaves the runtime file to the turn loop it actually owns.

use std::{num::NonZeroUsize, sync::Arc};

use rho_sdk::{
    model::Message, provider::ModelProvider, ApprovalHandler, ApprovalRequestReceiver, SessionId,
    SessionOptions,
};

use crate::{
    credential_store::AppCredentialStore, permission::PermissionMode,
    session::Session as StoredSession,
};
use rho_providers::providers::{build_sdk_provider_with_source, UnavailableProvider};

pub(super) fn resolve_provider(
    unavailable_error: Option<rho_providers::model::ModelError>,
    sdk_options: &crate::app::sdk_config::SdkBootstrapOptions,
) -> anyhow::Result<Arc<dyn ModelProvider>> {
    match unavailable_error {
        Some(error) => Ok(Arc::new(UnavailableProvider::new(error))),
        None => {
            let credentials =
                rho_providers::auth::provider_credentials::ApplicationCredentialSource::new(
                    Arc::new(AppCredentialStore),
                );
            Ok(build_sdk_provider_with_source(
                sdk_options.provider.clone(),
                &credentials,
            )?)
        }
    }
}

pub(super) fn resolve_session_options(
    provider: &Arc<dyn ModelProvider>,
    history: Vec<Message>,
    session_id: Option<&str>,
    storage: Option<&StoredSession>,
) -> anyhow::Result<SessionOptions> {
    let cache_key = session_id.map(prompt_cache_key);
    let resumed_snapshot = storage
        .map(|storage| {
            storage.snapshot_for_resume(
                provider.identity(),
                cache_key
                    .clone()
                    .unwrap_or_else(|| prompt_cache_key(storage.id())),
            )
        })
        .transpose()?;
    if let Some(snapshot) = resumed_snapshot {
        // The TUI has not started yet, so stderr is still safe here.
        if let Some(notice) = resume_omissions_notice(&snapshot, &provider.identity()) {
            eprintln!("warning: {notice}");
        }
        return Ok(SessionOptions::from_snapshot(snapshot));
    }
    // Always seed a prompt-cache key, including brand-new sessions that
    // do not yet have durable storage. ensure_session later reuses this
    // session id when creating the on-disk transcript.
    let id = match session_id {
        Some(id) => SessionId::from_string(id)?,
        None => SessionId::new(),
    };
    Ok(SessionOptions::new()
        .history(history)
        .id(id.clone())
        .prompt_cache_key(prompt_cache_key(id.as_str())))
}

pub(super) fn approval_channel_for(
    mode: PermissionMode,
) -> (
    Option<Arc<dyn ApprovalHandler>>,
    Option<ApprovalRequestReceiver>,
) {
    match mode {
        PermissionMode::Supervised => {
            let capacity = NonZeroUsize::new(16).expect("approval channel capacity is non-zero");
            let (handler, receiver) = rho_sdk::approval_channel(capacity);
            (Some(Arc::new(handler)), Some(receiver))
        }
        PermissionMode::Auto | PermissionMode::Plan => (None, None),
    }
}

pub(super) fn prompt_cache_key(id: &str) -> String {
    rho_providers::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

pub(super) fn resume_omissions_report(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<rho_sdk::model::handoff::HandoffReport> {
    let report = snapshot.provider_context_omissions(target);
    report.has_omissions().then_some(report)
}

fn resume_omissions_notice(
    snapshot: &rho_sdk::SessionSnapshot,
    target: &rho_sdk::model::ModelIdentity,
) -> Option<String> {
    resume_omissions_report(snapshot, target).map(|report| {
        format!(
            "omitted {} incompatible provider-native context block(s) while resuming session (kinds: {})",
            report.omitted_provider_context,
            report.omitted_kinds.join(", ")
        )
    })
}
