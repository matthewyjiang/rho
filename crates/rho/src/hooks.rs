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
//! - a hook child gets a fixed base environment plus its own allowlist, read
//!   from its session's environment (#1659), never the whole of it.
//!
//! A hook cannot widen anything. `before_tool_use` may only let the existing
//! decision stand or deny it.

mod activity;
mod catalog;
mod command;
mod config;
mod configured;
mod diagnostics;
mod dispatch;
mod matcher;
mod protocol;

use std::sync::Arc;

use crate::{
    session_environment::{Ambient, SessionEnvironment},
    workspace::ProjectTrusts,
};

pub(crate) use catalog::{check_user_hooks, user_hooks_path};
pub use catalog::{HookCatalog, TRUST_PROJECT_HOOKS_ENV};
pub use config::HookConfigError;
pub(crate) use diagnostics::{HookContractView, HookDrift};
pub use diagnostics::{HookInspector, HookReport, HookSnapshot};
pub use dispatch::HookEngine;
pub(crate) use dispatch::WorkflowNodeFinished;

/// Marker that tells a nested Rho it is running inside a hook program.
///
/// A hook is trusted automation, and trusted automation may legitimately call
/// `rho`. What it must not do is trigger the hooks that invoked it, so the child
/// sees this marker and runs with hooks disabled. A daemon session sees it in
/// the environment of the client that opened it (#1659).
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
    /// Starts the hook pipeline of a session whose environment is
    /// `environment`, which its hooks run in, or returns `None` when nothing is
    /// configured or the session is a hook program's own.
    ///
    /// A session without hooks then pays nothing: no gate, no observer, and no
    /// hook machinery in diagnostics. A session with only observational hooks
    /// does not install a pre-tool gate; a session with only blocking hooks does
    /// not start an observational worker.
    ///
    /// Recursion is decided from the session's environment, not this
    /// process's: a daemon session that a hook's `rho` opened runs without
    /// hooks, while the daemon's other sessions keep theirs (#1659).
    pub fn start(
        catalog: HookCatalog,
        environment: SessionEnvironment,
        cancellation: rho_sdk::CancellationToken,
    ) -> Option<Self> {
        if catalog.is_empty() || running_inside_hook(&environment.current()) {
            return None;
        }
        let has_blocking = catalog.has_blocking_hooks();
        let has_observational = catalog.has_observational_hooks();
        let engine = Arc::new(HookEngine::new(
            catalog,
            rho_sdk::hooks::HookPayloadBounds::default(),
            environment,
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

    /// Installs this pipeline's gate and observer on a provider-free tool host.
    pub fn attach_tool_host(
        &self,
        mut builder: rho_sdk::ToolHostBuilder,
    ) -> rho_sdk::ToolHostBuilder {
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
    /// reload can never change the hook set halfway through one decision. On an
    /// error the previous hook set stays in force.
    ///
    /// Reload cannot install a gate or observer that was absent at start, so a
    /// catalog that needs one is refused (see [`HookPorts`]).
    pub fn reload_for_cwd(&self, cwd: &std::path::Path) -> Result<(), HookReloadError> {
        let mut discard = |_message: String| {};
        let catalog = discover_for_cwd(cwd, self.engine.project_trust(), &mut discard)?;
        if let Some(kind) = HookPorts::of(Some(self)).missing(&catalog) {
            return Err(HookReloadError::NeedsReopen(kind));
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

/// Which kinds of hook a session can run. Each kind needs a port the
/// session's SDK runtime installs when it is built (the gate for blocking
/// hooks, the observer for observing ones), so a kind the session started
/// without runs only once it is opened again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HookPorts {
    blocking: bool,
    observing: bool,
}

impl HookPorts {
    /// The ports of a session running `hooks`; none without a hook runtime.
    pub(crate) fn of(hooks: Option<&HookPipeline>) -> Self {
        hooks.map_or_else(Self::default, |hooks| Self {
            blocking: hooks.gate.is_some(),
            observing: hooks.observer.is_some(),
        })
    }

    /// A kind of hook `catalog` has that cannot run here.
    pub(crate) fn missing(self, catalog: &HookCatalog) -> Option<HookKind> {
        if catalog.has_blocking_hooks() && !self.blocking {
            Some(HookKind::Blocking)
        } else if catalog.has_observational_hooks() && !self.observing {
            Some(HookKind::Observing)
        } else {
            None
        }
    }
}

/// A kind of hook, by the port it runs through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookKind {
    /// `before_tool_use` hooks, which may deny what follows.
    Blocking,
    /// Every other hook, which only hears of what happened.
    Observing,
}

impl std::fmt::Display for HookKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Blocking => "blocking",
            Self::Observing => "observing",
        })
    }
}

/// Why a session's hooks were not reloaded; it keeps the hooks it ran.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HookReloadError {
    /// A hooks file that loads could not be read or validated.
    #[error(transparent)]
    Invalid(#[from] HookConfigError),
    /// The files add a kind of hook the session started without.
    #[error(
        "{0} hooks were added since this session started; open the session again to load them"
    )]
    NeedsReopen(HookKind),
    /// A hook program started the session, which runs no hooks (#1659).
    #[error("a hook started this session, so it runs no hooks")]
    InsideHook,
}

/// Whether `environment` is a hook program's: [`IN_HOOK_ENV`] is set in it.
///
/// A hook may legitimately run `rho`; it must not make that nested Rho, or a
/// daemon session it opens, fire the hooks that invoked it (#1659).
pub fn running_inside_hook(environment: &Ambient) -> bool {
    environment.read_os(IN_HOOK_ENV).is_some()
}

/// Reports the hooks of a session running `hooks` in the workspace
/// containing `cwd`: its hooks files read again and checked against what it
/// runs, without changing what it runs. `inside_hook` is
/// [`running_inside_hook`] for the session's environment, and `trust` its
/// project trust.
///
/// A file that does not load is reported, not raised: the report shows it
/// with the hooks the session runs.
pub fn read_snapshot(
    hooks: Option<&HookPipeline>,
    cwd: &std::path::Path,
    trust: ProjectTrusts,
    inside_hook: bool,
) -> HookSnapshot {
    // A Rho a hook started runs without hooks (#1659).
    if hooks.is_none() && inside_hook {
        return HookSnapshot::disabled();
    }
    let mut discard = |_message: String| {};
    HookSnapshot::checked(hooks, discover_for_cwd(cwd, trust, &mut discard))
}

/// Reloads the hooks of a session running `hooks` in the workspace
/// containing `cwd` from their files, as `/hooks` asks
/// (`rho/session/hooks/reload`). `inside_hook` and `trust` are as for
/// [`read_snapshot`].
///
/// On an error the session keeps the hooks it ran (see
/// [`HookPipeline::reload_for_cwd`]). A session that started without hooks
/// cannot start them here: installing a gate or worker means rebuilding its
/// SDK runtime, so hooks added since load when it is opened again.
pub fn reload_for_session(
    hooks: Option<&HookPipeline>,
    cwd: &std::path::Path,
    trust: ProjectTrusts,
    inside_hook: bool,
) -> Result<(), HookReloadError> {
    if let Some(hooks) = hooks {
        return hooks.reload_for_cwd(cwd);
    }
    // A Rho a hook started runs without hooks (#1659).
    if inside_hook {
        return Err(HookReloadError::InsideHook);
    }
    let mut discard = |_message: String| {};
    let catalog = discover_for_cwd(cwd, trust, &mut discard)?;
    match HookPorts::default().missing(&catalog) {
        Some(kind) => Err(HookReloadError::NeedsReopen(kind)),
        None => Ok(()),
    }
}

/// How long queued observational hooks may finish after a session ends.
pub const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Starts hooks for the workspace containing `cwd`, run in `environment`, the
/// session's, and trusted as it trusts the project; none when the session is
/// a hook program's own (see [`HookPipeline::start`]).
///
/// A hooks file that fails validation must not take the session down. The
/// failure is logged and the session runs without hooks, which is the same
/// outcome as having none configured.
pub fn start_for_cwd(
    cwd: &std::path::Path,
    environment: SessionEnvironment,
) -> Option<HookPipeline> {
    let catalog = catalog_for_cwd(cwd, &environment)?;
    HookPipeline::start(catalog, environment, rho_sdk::CancellationToken::new())
}

/// [`start_for_cwd`] with the blocking gate alone, for a session Rho runs
/// for itself, such as the title agent's: its tool calls obey the user's
/// `before_tool_use` hooks, and no observational hook hears of a session
/// the user never started. `None` without a blocking hook.
pub(crate) fn start_gate_for_cwd(
    cwd: &std::path::Path,
    environment: SessionEnvironment,
) -> Option<HookPipeline> {
    let mut catalog = catalog_for_cwd(cwd, &environment)?;
    catalog.retain_blocking();
    HookPipeline::start(catalog, environment, rho_sdk::CancellationToken::new())
}

/// The hooks a session in `environment` starts for the workspace containing
/// `cwd`, or `None`, logged, when its hooks files do not load.
fn catalog_for_cwd(cwd: &std::path::Path, environment: &SessionEnvironment) -> Option<HookCatalog> {
    let mut report = |message: String| tracing::info!(target: "rho::hooks", "{message}");
    match discover_for_cwd(cwd, environment.project_trust(), &mut report) {
        Ok(catalog) => Some(catalog),
        Err(error) => {
            tracing::warn!(target: "rho::hooks", "hooks are disabled: {error}");
            None
        }
    }
}

/// Discovers the hooks that apply to the workspace containing `cwd`.
///
/// The project's hooks file loads only under `trust`'s hook trust, the
/// session's. Hook recursion is the caller's decision (see
/// [`running_inside_hook`]). Reports any skipped untrusted project file
/// through `report` so a host can show it once instead of on every dispatch.
pub fn discover_for_cwd(
    cwd: &std::path::Path,
    trust: ProjectTrusts,
    report: &mut dyn FnMut(String),
) -> Result<HookCatalog, HookConfigError> {
    let project_root = project_root(cwd);
    let rho_home = crate::paths::rho_dir().ok();
    let catalog = HookCatalog::discover(rho_home.as_deref(), Some(&project_root), trust.hooks)?;
    if let Some(skipped) = catalog.skipped_untrusted() {
        report(format!(
            "ignoring {} because this workspace is not trusted; set {TRUST_PROJECT_HOOKS_ENV}=1 to load it",
            crate::paths::display(&skipped.path)
        ));
    }
    Ok(catalog)
}

/// The hooks files a session in `cwd` would load under `trust`, each with
/// the hooks it defines, read without starting anything (`rho/hooks/list`):
/// the user's, then, given `cwd`, its project's.
pub(crate) fn configured_for_cwd(
    cwd: Option<&std::path::Path>,
    trust: ProjectTrusts,
) -> Vec<rho_protocol::HooksFile> {
    let rho_home = crate::paths::rho_dir().ok();
    let project_root = cwd.map(project_root);
    configured::configured_files(rho_home.as_deref(), project_root.as_deref(), trust.hooks)
}

/// The project whose `.rho/hooks.toml` applies to `cwd`: its repository's
/// root, else `cwd` itself.
fn project_root(cwd: &std::path::Path) -> std::path::PathBuf {
    crate::workspace::project_ancestor_dirs(cwd)
        .into_iter()
        .next()
        .unwrap_or_else(|| cwd.to_path_buf())
}

#[cfg(test)]
#[path = "hooks/hooks_tests.rs"]
mod tests;
