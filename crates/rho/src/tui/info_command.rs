use std::path::PathBuf;

use ratatui::text::Line;
use rho_providers::model::{ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage};

use super::{
    command_block::CommandBlock,
    usage_cost::{estimated_cost_usd_micros, format_usd},
    workspace::git_branch,
    App, Entry,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BillingInfo {
    Metered,
    Subscription,
}

impl BillingInfo {
    fn from_provider_auth(provider: &str, auth: &str) -> Self {
        if provider == "openai-codex" || auth == "codex" || auth == "xai-oauth" {
            Self::Subscription
        } else {
            Self::Metered
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Metered => "metered API",
            Self::Subscription => "subscription",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeInfo {
    version: String,
    provider: String,
    model: String,
    reasoning: String,
    permission_mode: String,
    billing: BillingInfo,
    cwd: PathBuf,
    branch: Option<String>,
    usage: Option<ModelUsage>,
    latest_usage: Option<ModelUsage>,
    context_usage: Option<ContextUsage>,
    model_metadata: Option<ModelMetadata>,
}

impl App {
    pub(super) fn execute_info_command(&mut self) -> anyhow::Result<()> {
        let identity = self.info.services.diagnostics.identity();
        let info = RuntimeInfo {
            version: identity.rho_version.to_string(),
            provider: identity.provider.to_string(),
            model: identity.model.to_string(),
            reasoning: identity.reasoning.to_string(),
            permission_mode: self.info.runtime.permission_mode.as_str().into(),
            billing: BillingInfo::from_provider_auth(
                &self.info.runtime.provider,
                &self.info.runtime.auth,
            ),
            cwd: self.info.runtime.cwd.clone(),
            branch: git_branch(&self.info.runtime.cwd),
            usage: self.cumulative_usage.clone(),
            latest_usage: self.latest_usage.clone(),
            context_usage: self.current_context.clone(),
            model_metadata: self.model_metadata.clone(),
        };
        self.insert_entry(&Entry::RuntimeInfo(Box::new(info)));
        self.status = "runtime info".into();
        Ok(())
    }
}

pub(super) fn runtime_info_lines(info: &RuntimeInfo, width: usize) -> Vec<Line<'static>> {
    let mut block = CommandBlock::new(width);
    block.push_header("rho", &format!("v{}", info.version));

    block.push_section("Model");
    block.push_field("Provider", &info.provider);
    block.push_field("Model", &info.model);
    block.push_field("Reasoning", &info.reasoning);
    block.push_field("Permissions", &info.permission_mode);
    block.push_field("Billing", info.billing.description());

    block.push_section("Session usage");
    push_usage_fields(&mut block, info);

    block.push_section("Workspace");
    block.push_field("Directory", &info.cwd.display().to_string());
    block.push_field(
        "Git branch",
        info.branch.as_deref().unwrap_or("not in a Git worktree"),
    );
    block.finish()
}

fn push_usage_fields(block: &mut CommandBlock, info: &RuntimeInfo) {
    if let Some(context) = format_context(info) {
        block.push_field("Context", &context);
    } else {
        block.push_field("Context", "not reported");
    }

    let Some(usage) = info.usage.as_ref() else {
        block.push_note("No token usage recorded yet.");
        return;
    };

    push_optional_number(block, "Input tokens", usage.input_tokens);
    push_optional_number(block, "Output tokens", usage.output_tokens);
    push_optional_number(block, "Cache read", usage.cache_read_tokens);
    push_optional_number(block, "Cache write", usage.cache_write_tokens);
    if let Some(percent) = cache_hit_percent(info.latest_usage.as_ref()) {
        block.push_field("Cache hit", &format!("{percent:.1}% on the latest request"));
    }

    let reported_cost = usage.cost_usd_micros;
    let cost =
        reported_cost.or_else(|| estimated_cost_usd_micros(usage, info.model_metadata.as_ref()));
    if let Some(cost) = cost {
        let qualifier = if reported_cost.is_none() {
            " estimated"
        } else {
            ""
        };
        let equivalent = if info.billing == BillingInfo::Subscription {
            " API equivalent"
        } else {
            ""
        };
        block.push_field(
            "Cost",
            &format!("{}{qualifier}{equivalent}", format_usd(cost)),
        );
    }
}

fn push_optional_number(block: &mut CommandBlock, label: &str, value: Option<u64>) {
    if let Some(value) = value {
        block.push_field(label, &format_number(value));
    }
}

fn cache_hit_percent(usage: Option<&ModelUsage>) -> Option<f64> {
    let usage = usage?;
    let cache_read = usage.cache_read_tokens?;
    let prompt_tokens = usage
        .input_tokens
        .unwrap_or_default()
        .saturating_add(cache_read);
    (prompt_tokens > 0).then(|| cache_read as f64 * 100.0 / prompt_tokens as f64)
}

fn format_context(info: &RuntimeInfo) -> Option<String> {
    let window = info
        .context_usage
        .as_ref()
        .and_then(|usage| usage.context_window)
        .or_else(|| {
            info.model_metadata
                .as_ref()
                .and_then(ModelMetadata::display_context_window)
        })
        .filter(|window| *window > 0)?;
    let source = match info.context_usage.as_ref().map(|usage| usage.source) {
        Some(ContextUsageSource::Estimated) => "estimated",
        Some(ContextUsageSource::ProviderReported) => "provider reported",
        Some(ContextUsageSource::UnknownAfterCompaction) => "unknown after compaction",
        None => "model limit",
    };
    let Some(tokens) = info.context_usage.as_ref().and_then(|usage| usage.tokens) else {
        return Some(format!(
            "unknown / {} tokens ({source})",
            format_number(window)
        ));
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    Some(format!(
        "{} / {} tokens ({percent:.1}%, {source})",
        format_number(tokens),
        format_number(window)
    ))
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(ch);
    }
    formatted
}

#[cfg(test)]
#[path = "info_command_tests.rs"]
mod tests;
