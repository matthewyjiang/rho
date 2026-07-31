//! Lifecycle hooks: configuration, trust, and execution.
//!
//! The SDK owns the generic contract (event vocabulary, envelopes, decision
//! types, the gate trait). This module owns everything policy-shaped: where
//! hooks files live, when a project's hooks may run, what a hook program is
//! allowed to see, and how one is executed and bounded.
//!
//! # Files
//!
//! ```text
//! ~/.rho/hooks.toml           user hooks, always eligible
//! <project>/.rho/hooks.toml   project hooks, only when the workspace is trusted
//! ```
//!
//! Preferences stay in `config.toml`. Executable policy lives only here, because
//! "which model do I prefer" and "which programs may Rho run" are different
//! trust questions.
//!
//! # Security posture
//!
//! Hook programs run with the user's authority, outside any agent sandbox, as
//! trusted user automation. That is the whole point of a deny hook: it must be
//! able to see and judge what the agent is about to do. Three things keep that
//! honest:
//!
//! - project hooks stay inert until the workspace is trusted, and the resolved
//!   spawn contract is printable before trust is granted;
//! - project hooks must name their program by path, so granting trust cannot
//!   silently bind whatever `PATH` resolves to today;
//! - a hook child gets a fixed base environment plus its own allowlist, never
//!   the ambient environment.
//!
//! A hook cannot widen anything. `before_tool_use` may only let the existing
//! decision stand or deny it.

mod activity;
mod catalog;
mod command;
mod config;
mod diagnostics;
mod dispatch;
mod environment;
mod matcher;
mod protocol;
mod supervisor;

use std::sync::Arc;

pub use catalog::{HookCatalog, ProjectTrust, TRUST_PROJECT_HOOKS_ENV};
pub use config::HookConfigError;
pub use diagnostics::{contract_views, HookInspector, HookReport};
pub use dispatch::HookEngine;

/// Marker that tells a nested Rho it is running inside a hook program.
///
/// A hook is trusted automation, and trusted automation may legitimately call
/// `rho`. What it must not do is trigger the hooks that invoked it, so the child
/// sees this marker and runs with hooks disabled.
pub const IN_HOOK_ENV: &str = "RHO_IN_HOOK";

/// Which file declared a hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HookSource {
    User,
    Project,
}

impl HookSource {
    /// Namespace used in qualified hook IDs, so a project cannot break a user's
    /// configuration by shipping a colliding ID.
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// Host-owned hook pipeline: config engine, optional gate, optional worker.
///
/// One value per process, not per SDK runtime: the interactive host rebuilds its
/// runtime whenever permission mode or provider changes, and those rebuilds must
/// reuse the same engine, queue, and worker rather than starting new ones.
///
/// The gate is installed only when blocking hooks exist at start; the observer
/// only when observational hooks exist. Adding a class of hook that was absent
/// at session start still requires a restart, matching the empty-catalog case.
pub struct HookPipeline {
    engine: Arc<HookEngine>,
    gate: Option<Arc<dispatch::CommandHookGate>>,
    observer: Option<Arc<dispatch::QueuedHookObserver>>,
    worker: Option<dispatch::ObservationalWorker>,
}

impl HookPipeline {
    /// Starts the hook pipeline, or returns `None` when nothing is configured.
    ///
    /// A session without hooks then pays nothing: no gate, no observer, and no
    /// hook machinery in diagnostics. A session with only observational hooks
    /// does not install a pre-tool gate; a session with only blocking hooks does
    /// not start an observational worker.
    pub fn start(catalog: HookCatalog, cancellation: rho_sdk::CancellationToken) -> Option<Self> {
        if catalog.is_empty() {
            return None;
        }
        let has_blocking = catalog.has_blocking_hooks();
        let has_observational = catalog.has_observational_hooks();
        let engine = Arc::new(HookEngine::new(
            catalog,
            rho_sdk::hooks::HookPayloadBounds::default(),
        ));
        let (observer, worker) = if has_observational {
            let (observer, worker) =
                dispatch::observational_channel(Arc::clone(&engine), cancellation);
            (Some(Arc::new(observer)), Some(worker))
        } else {
            (None, None)
        };
        let gate =
            has_blocking.then(|| Arc::new(dispatch::CommandHookGate::new(Arc::clone(&engine))));
        Some(Self {
            gate,
            observer,
            engine,
            worker,
        })
    }

    /// Installs this pipeline's gate and observer on an SDK builder.
    pub fn attach(&self, mut builder: rho_sdk::RhoBuilder) -> rho_sdk::RhoBuilder {
        if let Some(gate) = &self.gate {
            builder = builder
                .pre_tool_gate_shared(Arc::clone(gate) as Arc<dyn rho_sdk::hooks::PreToolUseGate>);
        }
        if let Some(observer) = &self.observer {
            builder =
                builder.hook_observer_shared(
                    Arc::clone(observer) as Arc<dyn rho_sdk::hooks::HookObserver>
                );
        }
        builder
    }

    pub fn engine(&self) -> &Arc<HookEngine> {
        &self.engine
    }

    /// Re-reads the hooks files for the workspace containing `cwd`.
    ///
    /// The swap is atomic and applies to dispatches that have not started, so a
    /// reload can never change the hook set halfway through one decision. On a
    /// validation error the previous hook set stays in force.
    ///
    /// Reload cannot install a gate or observer that was absent at start. If the
    /// new catalog needs a port this session never attached, the caller should
    /// tell the user to restart.
    pub fn reload_for_cwd(&self, cwd: &std::path::Path) -> Result<(), HookConfigError> {
        let mut discard = |_message: String| {};
        let catalog = discover_for_cwd(cwd, &mut discard)?;
        if catalog.has_blocking_hooks() && self.gate.is_none() {
            return Err(HookConfigError::at_file(
                cwd,
                "blocking hooks were added since this session started; restart Rho to load them",
            ));
        }
        if catalog.has_observational_hooks() && self.observer.is_none() {
            return Err(HookConfigError::at_file(
                cwd,
                "observational hooks were added since this session started; restart Rho to load them",
            ));
        }
        self.engine.reload(catalog);
        Ok(())
    }

    /// Runs queued observational hooks for a bounded window, then stops.
    ///
    /// The caller must release every SDK runtime first; the worker finishes when
    /// the last observer sender is gone.
    pub async fn shutdown(mut self, grace: std::time::Duration) {
        drop(self.observer.take());
        if let Some(worker) = self.worker.take() {
            worker.drain(grace).await;
        }
    }
}

/// Whether this process is itself a hook program's child.
///
/// A hook may legitimately run `rho`; it must not make that nested Rho fire the
/// hooks that invoked it.
pub fn running_inside_hook() -> bool {
    std::env::var_os(IN_HOOK_ENV).is_some()
}

/// How long queued observational hooks may finish after a session ends.
pub const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Starts hooks for the workspace containing `cwd`.
///
/// A hooks file that fails validation must not take the session down. The
/// failure is logged and the session runs without hooks, which is the same
/// outcome as having none configured.
pub fn start_for_cwd(cwd: &std::path::Path) -> Option<HookPipeline> {
    let mut report = |message: String| tracing::info!(target: "rho::hooks", "{message}");
    match discover_for_cwd(cwd, &mut report) {
        Ok(catalog) => HookPipeline::start(catalog, rho_sdk::CancellationToken::new()),
        Err(error) => {
            tracing::warn!(target: "rho::hooks", "hooks are disabled: {error}");
            None
        }
    }
}

/// Discovers the hooks that apply to the workspace containing `cwd`.
///
/// Returns an empty catalog when hooks are suppressed by hook recursion. Reports
/// any skipped untrusted project file through `report` so a host can show it
/// once instead of on every dispatch.
pub fn discover_for_cwd(
    cwd: &std::path::Path,
    report: &mut dyn FnMut(String),
) -> Result<HookCatalog, HookConfigError> {
    if running_inside_hook() {
        return Ok(HookCatalog::default());
    }
    let project_root = crate::workspace::project_ancestor_dirs(cwd)
        .into_iter()
        .next();
    let rho_home = crate::paths::rho_dir().ok();
    let trust = ProjectTrust::from_env(std::env::var(TRUST_PROJECT_HOOKS_ENV).ok().as_deref());
    let catalog = HookCatalog::discover(rho_home.as_deref(), project_root.as_deref(), trust)?;
    if let Some(skipped) = catalog.skipped_untrusted() {
        report(format!(
            "ignoring {} because this workspace is not trusted; set {TRUST_PROJECT_HOOKS_ENV}=1 to load it",
            crate::paths::display(&skipped.path)
        ));
    }
    Ok(catalog)
}

#[cfg(test)]
#[path = "hooks/hooks_tests.rs"]
mod tests;
