#[cfg(test)]
use std::sync::Mutex;
use std::sync::{Arc, RwLock};

use serde::Serialize;

#[path = "diagnostics_compaction.rs"]
mod compaction;
pub(crate) use compaction::{
    CompactionContext, CompactionDiagnostics, IdleCompactionCheck, IdleCompactionReason,
};

use crate::compaction_metrics::{CompactionMetrics, CompactionRecord, ToolFingerprint};

use {
    crate::compaction::CompactionConfig, crate::config::Config, rho_providers::model::ContextUsage,
    rho_providers::reasoning::ReasoningLevel,
};

/// Actions the `rho` tool advertises. All but [`AGENTS_ACTION`] read the
/// [`RuntimeDiagnostics`] snapshot.
pub(crate) const ACTIONS: &[&str] = &[
    "info",
    "context",
    "compaction",
    "prompt_sources",
    "tools",
    "hooks",
    "config",
    AGENTS_ACTION,
];

/// Rediscovers agent definitions from disk for the session workspace, so the
/// `rho` tool answers it with the workspace instead of the snapshot.
pub(crate) const AGENTS_ACTION: &str = "agents";

pub(crate) fn supports_action(action: &str) -> bool {
    ACTIONS.contains(&action)
}

pub(crate) fn unsupported_action_error(action: &str) -> String {
    format!(
        "unknown rho diagnostics action '{action}'; expected one of: {}",
        ACTIONS.join(", ")
    )
}

#[cfg(test)]
pub fn test_diagnostics(provider: &str, model: &str) -> RuntimeDiagnostics {
    let config = Config {
        provider: provider.into(),
        model: model.into(),
        ..Config::default()
    };
    RuntimeDiagnostics::new(&config)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuntimeIdentity {
    pub rho_version: String,
    pub provider: String,
    pub model: String,
    pub reasoning: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_fingerprint: Option<String>,
}

impl RuntimeIdentity {
    pub fn new(provider: &str, model: &str, reasoning: ReasoningLevel) -> Self {
        Self {
            rho_version: env!("CARGO_PKG_VERSION").into(),
            provider: provider.into(),
            model: model.into(),
            reasoning: reasoning.to_string(),
            agent_id: None,
            agent_fingerprint: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SanitizedConfig {
    pub max_output_bytes: usize,
    pub max_tool_output_lines: usize,
    pub auto_compact: bool,
    pub compact_threshold_percent: u8,
    pub compact_target_percent: u8,
    pub web_search_mode: String,
    pub web_search_backend: String,
    pub xai_image_generation: bool,
    pub edit_tool: String,
    pub check_for_updates: bool,
    pub enable_subagents: bool,
    pub agent_concurrency: usize,
    pub advisor_mode: bool,
    pub codemode_mode: String,
    pub rtk: bool,
    pub source: String,
}

impl From<&Config> for SanitizedConfig {
    fn from(config: &Config) -> Self {
        Self {
            max_output_bytes: config.max_output_bytes,
            max_tool_output_lines: config.max_tool_output_lines,
            auto_compact: config.auto_compact,
            compact_threshold_percent: config.compact_threshold_percent,
            compact_target_percent: config.compact_target_percent,
            web_search_mode: config.web_search.mode.as_str().into(),
            web_search_backend: config.web_search.backend.as_str().into(),
            xai_image_generation: config.xai_image_generation,
            edit_tool: config.edit_tool.as_str().into(),
            check_for_updates: config.check_for_updates,
            enable_subagents: config.enable_subagents,
            agent_concurrency: config.agent_concurrency,
            advisor_mode: config.advisor_mode,
            codemode_mode: config.codemode.mode.as_str().into(),
            rtk: config.rtk,
            source: "live values used by this process; restart-only settings may differ from saved config"
                .into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct RuntimeState {
    identity: RuntimeIdentity,
    context: Option<ContextUsage>,
    compaction: Option<CompactionDiagnostics>,
    /// Latest compaction record, kept apart so context refreshes do not drop it.
    #[serde(skip)]
    compaction_metrics: CompactionMetrics,
    prompt_sources: Vec<crate::prompt::PromptSource>,
    tools: Vec<String>,
    config: SanitizedConfig,
    /// Live hook state, absent until a session installs a hook runtime.
    #[serde(skip)]
    hooks: Option<crate::hooks::HookInspector>,
}

impl RuntimeState {
    /// Context refreshes rebuild `compaction`; the record lives apart and is
    /// joined only on read so it has one source of truth.
    fn compaction_with_metrics(&self) -> Option<CompactionDiagnostics> {
        self.compaction.clone().map(|mut compaction| {
            compaction.last_compaction = self.compaction_metrics.last().cloned();
            compaction
        })
    }
}

/// Where finished compaction records go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompactionLedger {
    /// The usage ledger, for `/spend` and the #1280 metrics.
    Usage,
    /// Nowhere. Offline replays must not mix into real usage.
    Off,
}

/// Whether a ledger write has to finish before the caller continues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompactionSave {
    /// Queued off the async runtime. May not finish if the process exits.
    Deferred,
    /// Runs to completion on the caller. Required on shutdown.
    Inline,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapturedCompaction {
    pub(crate) save: CompactionSave,
    pub(crate) record: CompactionRecord,
}

#[derive(Clone, Debug)]
pub struct RuntimeDiagnostics {
    state: Arc<RwLock<RuntimeState>>,
    ledger: CompactionLedger,
    /// Records handed to the ledger during tests. Production saves skip this.
    #[cfg(test)]
    saves: Arc<Mutex<Vec<CapturedCompaction>>>,
}

impl RuntimeDiagnostics {
    pub fn new(config: &Config) -> Self {
        Self::with_ledger(config, CompactionLedger::Usage)
    }

    /// Diagnostics whose compaction records are kept in memory only.
    pub(crate) fn without_ledger(config: &Config) -> Self {
        Self::with_ledger(config, CompactionLedger::Off)
    }

    fn with_ledger(config: &Config, ledger: CompactionLedger) -> Self {
        Self {
            ledger,
            #[cfg(test)]
            saves: Arc::new(Mutex::new(Vec::new())),
            state: Arc::new(RwLock::new(RuntimeState {
                identity: RuntimeIdentity::new(&config.provider, &config.model, config.reasoning),
                context: None,
                compaction: None,
                compaction_metrics: CompactionMetrics::default(),
                prompt_sources: Vec::new(),
                tools: Vec::new(),
                config: config.into(),
                hooks: None,
            })),
        }
    }

    fn save_compaction(&self, record: Option<CompactionRecord>) {
        self.persist(record, CompactionSave::Deferred);
    }

    /// Writes a follow-up that is still open so quitting mid-window keeps the
    /// partial re-read counts. The write is inline: the usual save is
    /// `spawn_blocking` and can be dropped when the process exits.
    pub(crate) fn flush_unfinished_compaction(&self) {
        let unfinished = self.write().compaction_metrics.take_unfinished();
        self.persist(unfinished, CompactionSave::Inline);
    }

    #[cfg(test)]
    pub(crate) fn captured_compactions(&self) -> Vec<CapturedCompaction> {
        self.saves
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn persist(&self, record: Option<CompactionRecord>, save: CompactionSave) {
        let Some(record) = record else {
            return;
        };
        #[cfg(test)]
        self.saves
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(CapturedCompaction {
                save,
                record: record.clone(),
            });
        match (self.ledger, save) {
            (CompactionLedger::Off, _) => {}
            (CompactionLedger::Usage, CompactionSave::Deferred) => {
                crate::usage::save_compaction(Some(record));
            }
            (CompactionLedger::Usage, CompactionSave::Inline) => {
                crate::usage::save_compaction_inline(Some(record));
            }
        }
    }

    pub fn identity(&self) -> RuntimeIdentity {
        self.read().identity.clone()
    }

    pub fn update_identity(&self, provider: &str, model: &str, reasoning: ReasoningLevel) {
        let mut state = self.write();
        let agent_id = state.identity.agent_id.clone();
        let agent_fingerprint = state.identity.agent_fingerprint.clone();
        state.identity = RuntimeIdentity::new(provider, model, reasoning);
        state.identity.agent_id = agent_id;
        state.identity.agent_fingerprint = agent_fingerprint;
        state.context = None;
        state.compaction = None;
        let unfinished = std::mem::take(&mut state.compaction_metrics).take_unfinished();
        drop(state);
        self.save_compaction(unfinished);
    }

    pub fn update_agent(&self, id: &str, fingerprint: &str) {
        let mut state = self.write();
        state.identity.agent_id = Some(id.to_string());
        state.identity.agent_fingerprint = Some(fingerprint.to_string());
    }

    pub fn record_context(&self, context: ContextUsage) {
        self.write().context = Some(context);
    }

    pub(crate) fn compaction(&self) -> Option<CompactionDiagnostics> {
        self.read().compaction_with_metrics()
    }

    pub(crate) fn clear_compaction(&self) {
        let mut state = self.write();
        state.compaction = None;
        let unfinished = std::mem::take(&mut state.compaction_metrics).take_unfinished();
        drop(state);
        self.save_compaction(unfinished);
    }

    pub(crate) fn record_compaction_context(
        &self,
        current: CompactionContext,
        last_provider_check: Option<rho_sdk::CompactionDecision>,
        completed: rho_sdk::CompactionState,
    ) {
        let mut state = self.write();
        let last_idle_check = state
            .compaction
            .as_mut()
            .and_then(|previous| previous.last_idle_check.take());
        state.compaction = Some(CompactionDiagnostics {
            current,
            last_idle_check,
            last_provider_check: last_provider_check.map(Into::into),
            last_compaction: None,
            completed,
        });
    }

    /// Records one compactor call and saves it to the usage ledger.
    /// Compactors call this, so it covers automatic, manual, and
    /// overflow-recovery compactions alike, whether they succeed or fail.
    /// `removed` identifies the tool calls whose results the replacement drops.
    pub(crate) fn record_compaction(
        &self,
        record: CompactionRecord,
        removed: std::collections::HashSet<ToolFingerprint>,
    ) {
        let saved = record.clone();
        let superseded = self.write().compaction_metrics.record(record, removed);
        self.save_compaction(superseded);
        self.save_compaction(Some(saved));
    }

    /// The latest compactor call's record.
    pub(crate) fn last_compaction(&self) -> Option<CompactionRecord> {
        self.read().compaction_metrics.last().cloned()
    }

    /// The SDK committed the latest compactor result; start its follow-up.
    pub(crate) fn compaction_committed(&self) {
        self.write().compaction_metrics.committed();
    }

    /// Feeds the latest provider-reported prompt size to the follow-up.
    pub(crate) fn observe_prompt_tokens(&self, tokens: Option<u64>) {
        let finished = self
            .write()
            .compaction_metrics
            .observe_prompt_tokens(tokens);
        self.save_compaction(finished);
    }

    /// Feeds a proposed tool call to the follow-up.
    pub(crate) fn observe_tool_call(&self, call: &rho_sdk::model::ToolCall) {
        let finished = self.write().compaction_metrics.observe_tool_call(call);
        self.save_compaction(finished);
    }

    pub(crate) fn record_idle_compaction(&self, check: IdleCompactionCheck) {
        if let Some(compaction) = self.write().compaction.as_mut() {
            compaction.last_idle_check = Some(check);
        }
    }

    pub(crate) fn update_web_search(&self, settings: &crate::config::WebSearchSettings) {
        let mut state = self.write();
        state.config.web_search_mode = settings.mode.as_str().into();
        state.config.web_search_backend = settings.backend.as_str().into();
    }

    pub fn update_compaction_config(&self, config: &CompactionConfig) {
        let mut state = self.write();
        state.config.auto_compact = config.auto_compact;
        state.config.compact_threshold_percent = config.threshold_percent;
        state.config.compact_target_percent = config.target_percent;
    }

    pub fn update_max_tool_output_lines(&self, max_tool_output_lines: usize) {
        self.write().config.max_tool_output_lines = max_tool_output_lines;
    }

    pub fn update_check_for_updates(&self, check_for_updates: bool) {
        self.write().config.check_for_updates = check_for_updates;
    }

    /// Advisor mode applies to the next turn rather than the next process, so
    /// the mirror follows every change instead of the startup value.
    pub fn update_advisor_mode(&self, advisor_mode: bool) {
        self.write().config.advisor_mode = advisor_mode;
    }

    /// `/codemode` applies to the next model request, so the mirror follows
    /// every change instead of the startup value.
    pub fn update_codemode_mode(&self, mode: crate::config::CodemodeMode) {
        self.write().config.codemode_mode = mode.as_str().into();
    }

    /// Agent concurrency can change mid-session, so the mirror follows the live
    /// pool rather than the process startup snapshot.
    pub fn update_agent_concurrency(&self, agent_concurrency: usize) {
        self.write().config.agent_concurrency = agent_concurrency;
    }

    /// Edit tool selection can change mid-session, so the mirror follows the
    /// live value rather than the process startup snapshot.
    pub fn update_edit_tool(&self, edit_tool: &str) {
        self.write().config.edit_tool = edit_tool.into();
    }

    pub fn update_prompt_sources(&self, sources: Vec<crate::prompt::PromptSource>) {
        self.write().prompt_sources = sources;
    }

    #[cfg(test)]
    pub(crate) fn prompt_sources(&self) -> Vec<crate::prompt::PromptSource> {
        self.read().prompt_sources.clone()
    }

    /// Publishes the hook runtime so `rho(action="hooks")` can read live state.
    pub fn attach_hooks(&self, hooks: &crate::hooks::HookPipeline) {
        self.write().hooks = Some(crate::hooks::HookInspector::new(hooks));
    }

    pub fn update_tools(&self, tools: &[rho_tools::tool::ToolSpec]) {
        let mut tools = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        tools.sort();
        self.write().tools = tools;
    }

    pub fn response(&self, action: &str) -> Result<String, String> {
        let state = self.read();
        let value = match action {
            "info" => serde_json::to_value(&state.identity),
            "context" => serde_json::to_value(&state.context),
            "compaction" => serde_json::to_value(state.compaction_with_metrics()),
            "prompt_sources" => serde_json::to_value(&state.prompt_sources),
            "tools" => serde_json::to_value(&state.tools),
            "config" => serde_json::to_value(&state.config),
            "hooks" => serde_json::to_value(
                state
                    .hooks
                    .as_ref()
                    .map(crate::hooks::HookInspector::report)
                    .unwrap_or_else(crate::hooks::HookReport::disabled),
            ),
            _ => return Err(unsupported_action_error(action)),
        }
        .map_err(|error| error.to_string())?;
        serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, RuntimeState> {
        self.state.read().unwrap_or_else(|error| error.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, RuntimeState> {
        self.state
            .write()
            .unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
