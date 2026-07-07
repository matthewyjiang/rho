use std::{
    fs,
    path::{Path, PathBuf},
};

use ratatui::text::{Line, Span};

use super::{
    render::{display_width, truncate_one_line},
    theme::Theme,
    TuiInfo,
};
use crate::model::{ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusLineState {
    pub(super) cwd: PathBuf,
    pub(super) branch: Option<String>,
    pub(super) status: String,
    pub(super) usage: Option<ModelUsage>,
    pub(super) latest_usage: Option<ModelUsage>,
    pub(super) context_usage: Option<ContextUsage>,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) reasoning: String,
    pub(super) billing: BillingInfo,
    pub(super) model_metadata: Option<ModelMetadata>,
    pub(super) model_metadata_loading: bool,
}

impl StatusLineState {
    pub(super) fn from_tui(
        info: &TuiInfo,
        status: impl Into<String>,
        usage: Option<ModelUsage>,
        latest_usage: Option<ModelUsage>,
        context_usage: Option<ContextUsage>,
        model_metadata: Option<ModelMetadata>,
        model_metadata_loading: bool,
    ) -> Self {
        Self {
            cwd: info.cwd.clone(),
            branch: git_branch(&info.cwd),
            status: status.into(),
            usage,
            latest_usage,
            context_usage,
            provider: info.provider.clone(),
            model: info.model.clone(),
            reasoning: info.reasoning.to_string(),
            billing: BillingInfo::from_provider_auth(&info.provider, &info.auth),
            model_metadata,
            model_metadata_loading,
        }
    }

    fn left_top(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{} ({branch})", compact_cwd(&self.cwd)),
            None => compact_cwd(&self.cwd),
        }
    }

    fn right_bottom(&self) -> String {
        format!("({}) {} • {}", self.provider, self.model, self.reasoning)
    }
}

pub(super) fn statusline_lines(state: &StatusLineState, width: usize) -> Vec<Line<'static>> {
    vec![
        render_row(state.left_top(), String::new(), width),
        render_row(format_usage(state), state.right_bottom(), width),
        render_row(state.status.clone(), String::new(), width),
    ]
}

fn format_usage(state: &StatusLineState) -> String {
    if state.model_metadata_loading {
        return "querying models.dev".into();
    }

    let usage = state.usage.as_ref();
    let mut parts = Vec::new();
    if let Some(usage) = usage {
        if let Some(tokens) = usage.input_tokens {
            parts.push(format!("↑{}", compact_number(tokens)));
        }
        if let Some(tokens) = usage.output_tokens {
            parts.push(format!("↓{}", compact_number(tokens)));
        }
        if let Some(tokens) = usage.cache_read_tokens {
            parts.push(format!("R{}", compact_number(tokens)));
        }
        if let Some(tokens) = usage.cache_write_tokens {
            parts.push(format!("W{}", compact_number(tokens)));
        }
        if let Some(percent) = cache_hit_percent(state.latest_usage.as_ref()) {
            parts.push(format!("CH{percent:.1}%"));
        }
        if let Some(cost) = usage
            .cost_usd_micros
            .or_else(|| estimated_cost_usd_micros(usage, state.model_metadata.as_ref()))
        {
            parts.push(format!("${}", format_usd(cost)));
        }
    } else if state.model_metadata.as_ref().is_some_and(has_pricing) {
        parts.push(format!("${}", format_usd(0)));
    }
    if let Some(label) = state.billing.label() {
        parts.push(format!("({label})"));
    }
    if let Some(context) = format_context_usage(
        state.context_usage.as_ref(),
        state
            .model_metadata
            .as_ref()
            .and_then(ModelMetadata::display_context_window),
    ) {
        parts.push(context);
    }
    parts.join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BillingInfo {
    Metered,
    Subscription,
}

impl BillingInfo {
    fn from_provider_auth(provider: &str, auth: &str) -> Self {
        if provider == "openai-codex" || auth == "codex" {
            Self::Subscription
        } else {
            Self::Metered
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Self::Metered => None,
            Self::Subscription => Some("sub"),
        }
    }
}

fn has_pricing(metadata: &ModelMetadata) -> bool {
    metadata.cost_default.is_some() || metadata.cost_long_context.is_some()
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

fn format_context_usage(
    context_usage: Option<&ContextUsage>,
    metadata_context_window: Option<u64>,
) -> Option<String> {
    let window = context_usage
        .and_then(|usage| usage.context_window)
        .or(metadata_context_window)
        .filter(|window| *window > 0)?;
    let marker = match context_usage.map(|usage| usage.source) {
        Some(ContextUsageSource::Estimated) => "~",
        Some(ContextUsageSource::ProviderReported) | None => "",
        Some(ContextUsageSource::UnknownAfterCompaction) => "?",
    };
    let Some(tokens) = context_usage.and_then(|usage| usage.tokens) else {
        return Some(format!("{marker}/{}", compact_number(window)));
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    Some(format!("{marker}{percent:.1}%/{}", compact_number(window)))
}

fn estimated_cost_usd_micros(usage: &ModelUsage, metadata: Option<&ModelMetadata>) -> Option<u64> {
    let metadata = metadata?;
    let input = usage.input_tokens.unwrap_or_default();
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let total_input = usage.total_input_tokens().unwrap_or_default();
    let cost = metadata.cost_for_input_tokens(total_input)?;
    let mut micros = 0u128;
    micros += cost_component(input, cost.input_micros_per_m);
    micros += cost_component(
        usage.output_tokens.unwrap_or_default(),
        cost.output_micros_per_m,
    );
    micros += cost_component(cache_read, cost.cache_read_micros_per_m);
    micros += cost_component(
        usage.cache_write_tokens.unwrap_or_default(),
        cost.cache_write_micros_per_m,
    );
    (micros > 0).then_some(micros.min(u64::MAX as u128) as u64)
}

fn cost_component(tokens: u64, micros_per_million: Option<u64>) -> u128 {
    tokens as u128 * micros_per_million.unwrap_or_default() as u128 / 1_000_000
}

fn format_usd(micros: u64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    if dollars >= 100.0 {
        format!("{dollars:.0}")
    } else if dollars >= 10.0 {
        format!("{dollars:.2}")
    } else {
        format!("{dollars:.3}")
    }
}

fn compact_number(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn render_row(left: String, right: String, width: usize) -> Line<'static> {
    let style = Theme::dim();
    if right.is_empty() {
        return Line::from(Span::styled(truncate_one_line(&left, width), style));
    }

    let left_width = display_width(&left);
    let right_width = display_width(&right);
    if left_width + right_width < width {
        let gap = " ".repeat(width - left_width - right_width);
        return Line::from(Span::styled(format!("{left}{gap}{right}"), style));
    }

    let right_budget = right_width.min(width.saturating_div(2).max(1));
    let right = truncate_one_line(&right, right_budget);
    let right_width = display_width(&right);
    let left = truncate_one_line(&left, width.saturating_sub(right_width + 1).max(1));
    let left_width = display_width(&left);
    let gap = " ".repeat(width.saturating_sub(left_width + right_width));
    Line::from(Span::styled(format!("{left}{gap}{right}"), style))
}

fn git_branch(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/")
        .map(ToString::to_string)
        .or_else(|| head.get(..7).map(ToString::to_string))
}

fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let contents = fs::read_to_string(&dot_git).ok()?;
            let path = contents.trim().strip_prefix("gitdir: ")?;
            let path = Path::new(path);
            return Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                dir.join(path)
            });
        }
    }
    None
}

fn compact_cwd(path: &Path) -> String {
    let Some(home) = crate::paths::home_dir() else {
        return path.display().to_string();
    };

    if let Ok(rest) = path.strip_prefix(home) {
        let rel = rest.display().to_string();
        if rel.is_empty() {
            "~".to_string()
        } else {
            format!("~/{rel}")
        }
    } else {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::models_dev::ModelCost;

    fn priced_metadata() -> ModelMetadata {
        ModelMetadata {
            cost_default: Some(ModelCost {
                input_micros_per_m: Some(1_000_000),
                output_micros_per_m: Some(2_000_000),
                cache_read_micros_per_m: Some(100_000),
                cache_write_micros_per_m: None,
            }),
            ..ModelMetadata::default()
        }
    }

    fn test_state(usage: ModelUsage) -> StatusLineState {
        StatusLineState {
            cwd: PathBuf::from("/tmp/project"),
            branch: None,
            status: "idle".into(),
            usage: Some(usage.clone()),
            latest_usage: Some(usage),
            context_usage: None,
            provider: "openai".into(),
            model: "gpt-test".into(),
            reasoning: "low".into(),
            billing: BillingInfo::Metered,
            model_metadata: Some(priced_metadata()),
            model_metadata_loading: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn statusline_rows_use_display_width_for_alignment() {
        let line = render_row("项目".into(), "模型".into(), 10);
        let text = line_text(&line);

        assert_eq!(display_width(&text), 10);
    }

    #[test]
    fn estimated_statusline_cost_uses_normalized_input_and_cache_read() {
        let usage = ModelUsage {
            input_tokens: Some(300_000),
            cache_read_tokens: Some(700_000),
            output_tokens: Some(100_000),
            ..ModelUsage::default()
        };

        assert_eq!(
            estimated_cost_usd_micros(&usage, Some(&priced_metadata())),
            Some(570_000)
        );
    }

    #[test]
    fn cache_hit_percentage_uses_latest_usage_prompt_tokens() {
        let usage = ModelUsage {
            input_tokens: Some(300_000),
            cache_read_tokens: Some(700_000),
            output_tokens: Some(100_000),
            ..ModelUsage::default()
        };

        let formatted = format_usage(&test_state(usage));

        assert!(formatted.contains("↑300.0k"), "{formatted}");
        assert!(formatted.contains("R700.0k"), "{formatted}");
        assert!(formatted.contains("CH70.0%"), "{formatted}");
        assert!(formatted.contains("$0.570"), "{formatted}");
    }

    #[test]
    fn cache_hit_percentage_uses_latest_usage_not_cumulative_totals() {
        let mut state = test_state(ModelUsage {
            input_tokens: Some(1_000_000),
            cache_read_tokens: Some(1_000_000),
            output_tokens: Some(100_000),
            cache_write_tokens: Some(500_000),
            ..ModelUsage::default()
        });
        state.latest_usage = Some(ModelUsage {
            input_tokens: Some(100_000),
            cache_read_tokens: Some(900_000),
            cache_write_tokens: Some(500_000),
            ..ModelUsage::default()
        });

        let formatted = format_usage(&state);

        assert!(formatted.contains("↑1.0M"), "{formatted}");
        assert!(formatted.contains("R1.0M"), "{formatted}");
        assert!(formatted.contains("W500.0k"), "{formatted}");
        assert!(formatted.contains("CH90.0%"), "{formatted}");
        assert!(!formatted.contains("CH40.0%"), "{formatted}");
        assert!(!formatted.contains("CH60.0%"), "{formatted}");
    }

    #[test]
    fn context_percentage_uses_current_context_not_cumulative_usage() {
        let mut state = test_state(ModelUsage {
            input_tokens: Some(60_000),
            output_tokens: Some(40_000),
            ..ModelUsage::default()
        });
        state.context_usage = Some(ContextUsage::estimated(10_000, Some(100_000)));
        state.model_metadata = Some(ModelMetadata {
            advertised_context_window: Some(100_000),
            ..priced_metadata()
        });

        let formatted = format_usage(&state);

        assert!(formatted.contains("~10.0%/100.0k"), "{formatted}");
        assert!(!formatted.contains("100.0%/100.0k"), "{formatted}");
    }

    #[test]
    fn provider_reported_context_omits_estimate_marker() {
        let mut state = test_state(ModelUsage::default());
        state.context_usage = Some(ContextUsage::provider_reported(25_000, Some(100_000)));

        let formatted = format_usage(&state);

        assert!(formatted.contains("25.0%/100.0k"), "{formatted}");
        assert!(!formatted.contains("~25.0%/100.0k"), "{formatted}");
    }
}
