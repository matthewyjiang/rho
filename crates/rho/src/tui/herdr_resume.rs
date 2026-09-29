//! Builds the command Herdr runs to reopen this Rho session after a Herdr
//! server restart.
//!
//! A plain `rho --resume <id>` takes model, auth, reasoning, and permission
//! mode from config. The command pins only the values where this session
//! differs from config. Pinning everything would make restore stricter than a
//! plain resume: an explicit `--model` on a cached-model provider fails startup
//! when the model list cannot be refreshed, for example before a local server
//! is back up.

use std::path::PathBuf;

use rho_providers::model::ReasoningCapabilities;
use rho_providers::reasoning::ReasoningLevel;

use super::{App, RuntimeModelView};
use crate::herdr::HerdrSession;
use crate::permission::PermissionMode;

/// Launch-only CLI options a resumed session must repeat to behave the same.
/// Everything else is either live runtime state or read from config.
#[derive(Clone, Debug, Default)]
pub struct ResumeLaunchOptions {
    /// Absolute `--config` path, when one was passed.
    pub config: Option<PathBuf>,
    /// `--agent` id, when one was passed.
    pub agent: Option<String>,
    /// The bound agent's definition. Its model policy is part of what a plain
    /// relaunch selects, so it is not drift.
    pub definition: Option<std::sync::Arc<crate::agent::AgentDefinition>>,
    pub no_system_prompt: bool,
    pub no_tools: bool,
    pub no_subagents: bool,
}

/// The selection a resumed session would get from config alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResumeSelection {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub reasoning: ReasoningLevel,
    pub permission_mode: PermissionMode,
}

impl ResumeSelection {
    fn from_runtime(runtime: &RuntimeModelView) -> Self {
        Self {
            provider: runtime.provider.clone(),
            model: runtime.model.clone(),
            auth: runtime.auth.clone(),
            reasoning: runtime.reasoning,
            permission_mode: runtime.permission_mode,
        }
    }

    fn from_config(config: &crate::config::Config) -> Self {
        Self {
            provider: config.provider.clone(),
            model: config.model.clone(),
            auth: config.auth.clone(),
            reasoning: config.reasoning,
            permission_mode: config.permission_mode,
        }
    }
}

/// Everything the reported resume command depends on that can change while
/// Rho runs. A change means Herdr needs a fresh report. Config is tracked by
/// modification time so another pane's config edit is noticed without
/// reparsing config on every loop pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HerdrResumeKey {
    session_id: Option<String>,
    selection: ResumeSelection,
    config_modified: Option<std::time::SystemTime>,
}

/// Herdr sync state.
///
/// Herdr keeps the first session id a source reports and ignores a different
/// one until the pane is released, and it only accepts `report_agent_session`
/// once a `report_agent` claim holds the pane. So Rho tracks:
/// - `accepted`: the key Herdr last confirmed. Until a report succeeds it
///   differs from the current key, so failed or timed-out reports are retried.
/// - `held`: the session id Herdr may hold. Set by any sent report, even an
///   unconfirmed one, and cleared only by a confirmed release, so a switch
///   never reports a new id Herdr would ignore.
///
/// A confirmed release clears both, so a re-claim that fails is retried as a
/// claim.
#[derive(Clone, Debug, Default)]
pub(super) struct HerdrSync {
    built: Option<(HerdrResumeKey, Option<HerdrSession>)>,
    accepted: Option<HerdrResumeKey>,
    held: Option<String>,
}

/// How often an idle pane re-checks the resume command, so a config change
/// from another pane or a failed report does not wait for a keystroke. One
/// `stat` per wake; the argv is only rebuilt when something changed.
pub(super) const HERDR_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// What reconciling Herdr with the current key requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HerdrSyncStep {
    /// Herdr already has the current state.
    InSync,
    /// No claim is confirmed yet: claim the pane with the current session.
    Claim(Option<HerdrSession>),
    /// The pane is claimed with this session: update its resume command.
    Report(HerdrSession),
    /// Herdr may hold a different session. Release the pane, then claim it
    /// with the current session, if any.
    Reclaim(Option<HerdrSession>),
}

impl HerdrSync {
    fn step(&self, key: &HerdrResumeKey, session: Option<HerdrSession>) -> HerdrSyncStep {
        if self.accepted.as_ref() == Some(key) {
            return HerdrSyncStep::InSync;
        }
        let current = session.as_ref().map(|session| session.id.clone());
        match (self.held.as_ref(), session) {
            (Some(held), session) if current.as_ref() != Some(held) => {
                HerdrSyncStep::Reclaim(session)
            }
            (_, session) if self.accepted.is_none() => HerdrSyncStep::Claim(session),
            (_, Some(session)) => HerdrSyncStep::Report(session),
            (_, None) => HerdrSyncStep::InSync,
        }
    }

    /// Records a report sent with `session`. Herdr ignores a different id while
    /// it holds one, so only a first id is recorded.
    pub(super) fn note_sent(&mut self, session: Option<&HerdrSession>) {
        if self.held.is_none() {
            self.held = session.map(|session| session.id.clone());
        }
    }

    /// Records a confirmed release. Nothing is claimed afterwards, so the next
    /// sync must claim again rather than update a session Herdr dropped.
    pub(super) fn note_released(&mut self) {
        self.held = None;
        self.accepted = None;
    }
}

impl App {
    pub(super) fn herdr_resume_key(&self) -> HerdrResumeKey {
        let config_modified = self
            .info
            .services
            .config_repository
            .configured_path()
            .ok()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());
        HerdrResumeKey {
            session_id: self.info.session.session_id.clone(),
            selection: ResumeSelection::from_runtime(&self.info.runtime),
            config_modified,
        }
    }

    /// The Herdr session reference for the current state. Rebuilds only when
    /// the key changed.
    pub(super) fn herdr_session(&mut self) -> Option<HerdrSession> {
        self.current_herdr_session().1
    }

    pub(super) fn current_herdr_session(&mut self) -> (HerdrResumeKey, Option<HerdrSession>) {
        let key = self.herdr_resume_key();
        if let Some((built_key, session)) = &self.herdr_sync.built {
            if *built_key == key {
                return (key, session.clone());
            }
        }
        let session = self.build_herdr_session();
        self.herdr_sync.built = Some((key.clone(), session.clone()));
        (key, session)
    }

    /// Marks `key` as what Herdr now holds.
    pub(super) fn accept_herdr_key(&mut self, key: HerdrResumeKey) {
        self.herdr_sync.accepted = Some(key);
    }

    /// Accepts `key` after a state report Herdr answered, unless Herdr kept
    /// an earlier session and so ignored this one's id and resume command.
    pub(super) fn confirm_herdr_claim(
        &mut self,
        key: HerdrResumeKey,
        session: Option<&HerdrSession>,
        delivery: crate::herdr::HerdrDelivery,
    ) {
        let session_id = session.map(|session| session.id.as_str());
        match delivery {
            crate::herdr::HerdrDelivery::Accepted
                if self.herdr_sync.held.as_deref() == session_id =>
            {
                self.accept_herdr_key(key);
            }
            crate::herdr::HerdrDelivery::Accepted | crate::herdr::HerdrDelivery::Failed => {}
        }
    }

    /// The step that brings Herdr in line with the current state, and the key
    /// to accept once it succeeds.
    pub(super) fn herdr_sync_step(&mut self) -> (HerdrResumeKey, HerdrSyncStep) {
        let (key, session) = self.current_herdr_session();
        let step = self.herdr_sync.step(&key, session);
        (key, step)
    }

    fn build_herdr_session(&self) -> Option<HerdrSession> {
        let id = self.info.session.session_id.as_deref()?;
        let runtime = &self.info.runtime;
        let launch = &self.info.session.resume_launch;
        // Without readable config, pin everything rather than guess.
        let configured = self
            .info
            .services
            .config_repository
            .load()
            .ok()
            .map(|config| {
                let config = match &launch.definition {
                    Some(definition) => crate::app::relaunch_config(definition, &config),
                    None => config,
                };
                ResumeSelection::from_config(&config)
            });
        let reasoning_capabilities =
            rho_providers::model::models_dev::current_reasoning_capabilities(
                &runtime.provider,
                &runtime.model,
            );
        Some(HerdrSession {
            id: id.to_string(),
            resume_argv: Some(resume_argv(
                launch,
                &ResumeSelection::from_runtime(runtime),
                configured.as_ref(),
                &reasoning_capabilities,
                id,
            )),
        })
    }
}

/// `rho [launch options] [pinned selection] --resume <id>`. `configured` is
/// what config would select; values that match it are left to config.
pub(super) fn resume_argv(
    launch: &ResumeLaunchOptions,
    live: &ResumeSelection,
    configured: Option<&ResumeSelection>,
    reasoning_capabilities: &ReasoningCapabilities,
    session_id: &str,
) -> Vec<String> {
    let mut argv = vec!["rho".to_string()];
    if let Some(config) = &launch.config {
        argv.extend(["--config".into(), config.display().to_string()]);
    }
    if let Some(agent) = &launch.agent {
        argv.extend(["--agent".into(), agent.clone()]);
    }
    for (enabled, flag) in [
        (launch.no_system_prompt, "--no-system-prompt"),
        (launch.no_tools, "--no-tools"),
        (launch.no_subagents, "--no-subagents"),
    ] {
        if enabled {
            argv.push(flag.into());
        }
    }
    let differs = |same: fn(&ResumeSelection, &ResumeSelection) -> bool| {
        configured.is_none_or(|configured| !same(live, configured))
    };
    let pin_model = differs(|live, configured| {
        live.provider == configured.provider
            && live.model == configured.model
            && live.auth == configured.auth
    });
    if pin_model {
        argv.extend([
            "--model".into(),
            rho_providers::provider::model_reference(&live.provider, &live.model),
        ]);
        // `--auth` rejects the keyless marker; keyless providers resolve their
        // auth from the model reference alone.
        if live.auth != rho_providers::provider::KEYLESS_AUTH {
            argv.extend(["--auth".into(), live.auth.clone()]);
        }
    }
    // An explicit --reasoning fails startup on models without configurable
    // reasoning, so only pin it where a level can be chosen.
    let reasoning_configurable = match reasoning_capabilities {
        ReasoningCapabilities::Levels(_) | ReasoningCapabilities::Unknown => true,
        ReasoningCapabilities::NotConfigurable => false,
    };
    if reasoning_configurable
        && (pin_model || differs(|live, configured| live.reasoning == configured.reasoning))
    {
        argv.extend(["--reasoning".into(), live.reasoning.to_string()]);
    }
    if differs(|live, configured| live.permission_mode == configured.permission_mode) {
        argv.extend([
            "--permission-mode".into(),
            live.permission_mode.as_str().into(),
        ]);
    }
    argv.extend(["--resume".into(), session_id.into()]);
    argv
}

#[cfg(test)]
#[path = "herdr_resume_tests.rs"]
mod tests;
