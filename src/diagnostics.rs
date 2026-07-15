use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::{
    compaction::CompactionConfig, config::Config, model::ContextUsage, reasoning::ReasoningLevel,
};

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
}

impl RuntimeIdentity {
    pub fn new(provider: &str, model: &str, reasoning: ReasoningLevel) -> Self {
        Self {
            rho_version: env!("CARGO_PKG_VERSION").into(),
            provider: provider.into(),
            model: model.into(),
            reasoning: reasoning.to_string(),
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
    pub web_search_provider: String,
    pub check_for_updates: bool,
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
            web_search_provider: config.web_search_provider.as_str().into(),
            check_for_updates: config.check_for_updates,
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
    prompt_sources: Vec<crate::prompt::PromptSource>,
    tools: Vec<String>,
    config: SanitizedConfig,
}

#[derive(Clone, Debug)]
pub struct RuntimeDiagnostics {
    state: Arc<RwLock<RuntimeState>>,
}

impl RuntimeDiagnostics {
    pub fn new(config: &Config) -> Self {
        Self {
            state: Arc::new(RwLock::new(RuntimeState {
                identity: RuntimeIdentity::new(&config.provider, &config.model, config.reasoning),
                context: None,
                prompt_sources: Vec::new(),
                tools: Vec::new(),
                config: config.into(),
            })),
        }
    }

    pub fn identity(&self) -> RuntimeIdentity {
        self.read().identity.clone()
    }

    pub fn update_identity(&self, provider: &str, model: &str, reasoning: ReasoningLevel) {
        self.write().identity = RuntimeIdentity::new(provider, model, reasoning);
    }

    #[allow(dead_code)]
    pub fn record_context(&self, context: ContextUsage) {
        self.write().context = Some(context);
    }

    #[allow(dead_code)]
    pub fn clear_context(&self) {
        self.write().context = None;
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

    pub fn update_prompt_sources(&self, sources: Vec<crate::prompt::PromptSource>) {
        self.write().prompt_sources = sources;
    }

    pub fn update_tools(&self, tools: &[crate::tool::ToolSpec]) {
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
            "prompt_sources" => serde_json::to_value(&state.prompt_sources),
            "tools" => serde_json::to_value(&state.tools),
            "config" => serde_json::to_value(&state.config),
            _ => {
                return Err(format!(
                    "unknown rho diagnostics action '{action}'; load the rho-diagnostics skill for usage"
                ));
            }
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
