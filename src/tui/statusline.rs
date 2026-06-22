use std::{
    fs,
    path::{Path, PathBuf},
};

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::TuiInfo;
use crate::model::{ModelMetadata, ModelUsage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StatusLineState {
    pub(super) cwd: PathBuf,
    pub(super) branch: Option<String>,
    pub(super) status: String,
    pub(super) usage: Option<ModelUsage>,
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
        model_metadata: Option<ModelMetadata>,
        model_metadata_loading: bool,
    ) -> Self {
        Self {
            cwd: info.cwd.clone(),
            branch: git_branch(&info.cwd),
            status: status.into(),
            usage,
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
        if let (Some(cache_read), Some(input)) = (usage.cache_read_tokens, usage.input_tokens) {
            if input > 0 {
                let percent = cache_read as f64 * 100.0 / input as f64;
                parts.push(format!("CH{percent:.1}%"));
            }
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
    let context_window = usage.and_then(|usage| usage.context_window).or_else(|| {
        state
            .model_metadata
            .as_ref()
            .and_then(ModelMetadata::display_context_window)
    });
    if let Some(window) = context_window.filter(|window| *window > 0) {
        let total = usage.and_then(usage_total_tokens).unwrap_or_default();
        let percent = total as f64 * 100.0 / window as f64;
        parts.push(format!("{percent:.1}%/{}", compact_number(window)));
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

fn usage_total_tokens(usage: &ModelUsage) -> Option<u64> {
    usage.total_tokens.or_else(|| {
        add_numbers(
            usage.input_tokens.unwrap_or_default(),
            usage.output_tokens.unwrap_or_default(),
        )
    })
}

fn add_numbers(left: u64, right: u64) -> Option<u64> {
    let total = left.saturating_add(right);
    (total > 0).then_some(total)
}

fn estimated_cost_usd_micros(usage: &ModelUsage, metadata: Option<&ModelMetadata>) -> Option<u64> {
    let metadata = metadata?;
    let input = usage.input_tokens.unwrap_or_default();
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let uncached_input = input.saturating_sub(cache_read);
    let cost = metadata.cost_for_input_tokens(input)?;
    let mut micros = 0u128;
    micros += cost_component(uncached_input, cost.input_micros_per_m);
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
    let style = Style::default().fg(Color::DarkGray);
    if right.is_empty() {
        return Line::from(Span::styled(truncate_one_line(&left, width), style));
    }

    let left_width = left.chars().count();
    let right_width = right.chars().count();
    if left_width + right_width + 1 <= width {
        let gap = " ".repeat(width - left_width - right_width);
        return Line::from(Span::styled(format!("{left}{gap}{right}"), style));
    }

    let right_budget = right_width.min(width.saturating_div(2).max(1));
    let right = truncate_one_line(&right, right_budget);
    let right_width = right.chars().count();
    let left = truncate_one_line(&left, width.saturating_sub(right_width + 1).max(1));
    let left_width = left.chars().count();
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
    let Ok(home) = std::env::var("HOME") else {
        return path.display().to_string();
    };

    let home = Path::new(&home);
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

fn truncate_one_line(text: &str, width: usize) -> String {
    let mut text = text.replace('\n', " ");
    if text.chars().count() <= width {
        return text;
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    text = text.chars().take(width - 1).collect();
    text.push('…');
    text
}
