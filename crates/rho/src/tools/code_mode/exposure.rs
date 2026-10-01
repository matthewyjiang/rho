//! Pi-style tool exposure for Rho `codemode`.
//!
//! Modes: `direct` | `codemode` | `deferred` | `hidden`.
//! Defaults: core native tools → `direct`; MCP (`mcp__*`) → `codemode`.
//! Overrides: exact name beats pattern (first matching pattern wins by insertion order).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rho_sdk::model::ToolSpec;
use rho_sdk::tool::Tool;

/// How a tool may appear to the model vs `codemode` scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    /// Declared to the LLM as a normal tool.
    Direct,
    /// Callable from Starlark via ToolHost; not declared as a normal LLM tool.
    Codemode,
    /// Omitted from the LLM tool list until [`ExposureController::promote`].
    Deferred,
    /// Unreachable from the LLM and from `call_tool`.
    Hidden,
}

impl ToolExposure {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Codemode => "codemode",
            Self::Deferred => "deferred",
            Self::Hidden => "hidden",
        }
    }
}

/// One override: exact tool name, or glob-like `*` pattern (`mcp__computer__*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExposureOverride {
    Exact {
        name: String,
        exposure: ToolExposure,
    },
    Pattern {
        pattern: String,
        exposure: ToolExposure,
    },
}

impl ExposureOverride {
    pub fn exact(name: impl Into<String>, exposure: ToolExposure) -> Self {
        Self::Exact {
            name: name.into(),
            exposure,
        }
    }

    pub fn pattern(pattern: impl Into<String>, exposure: ToolExposure) -> Self {
        Self::Pattern {
            pattern: pattern.into(),
            exposure,
        }
    }

    fn matches(&self, tool_name: &str) -> Option<ToolExposure> {
        match self {
            Self::Exact { name, exposure } if name == tool_name => Some(*exposure),
            Self::Pattern { pattern, exposure } if glob_match(pattern, tool_name) => {
                Some(*exposure)
            }
            _ => None,
        }
    }
}

/// Policy: defaults + ordered overrides (exact checked before patterns among overrides).
#[derive(Debug, Clone, Default)]
pub struct ExposurePolicy {
    overrides: Vec<ExposureOverride>,
}

impl ExposurePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "override config waits on provider-boundary exposure; see docs/design/code-mode-starlark-v0.md checklist"
        )
    )]
    pub fn override_exact(mut self, name: impl Into<String>, exposure: ToolExposure) -> Self {
        self.overrides.push(ExposureOverride::exact(name, exposure));
        self
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "override config waits on provider-boundary exposure; see docs/design/code-mode-starlark-v0.md checklist"
        )
    )]
    pub fn override_pattern(mut self, pattern: impl Into<String>, exposure: ToolExposure) -> Self {
        self.overrides
            .push(ExposureOverride::pattern(pattern, exposure));
        self
    }

    /// Resolve exposure for a tool name.
    ///
    /// Exact overrides win over patterns; among patterns, first matching
    /// override in insertion order wins. Else MCP → `codemode`, else `direct`.
    pub fn resolve(&self, tool_name: &str) -> ToolExposure {
        let mut pattern_hit = None;
        for rule in &self.overrides {
            match rule {
                ExposureOverride::Exact { .. } => {
                    if let Some(exposure) = rule.matches(tool_name) {
                        return exposure;
                    }
                }
                ExposureOverride::Pattern { .. } => {
                    if pattern_hit.is_none() {
                        pattern_hit = rule.matches(tool_name);
                    }
                }
            }
        }
        if let Some(exposure) = pattern_hit {
            return exposure;
        }
        if is_mcp_tool_name(tool_name) {
            ToolExposure::Codemode
        } else {
            ToolExposure::Direct
        }
    }
}

pub fn is_mcp_tool_name(name: &str) -> bool {
    name.starts_with("mcp__")
}

/// Simple `*` glob: `*` matches any substring (including empty).
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    if !pattern.starts_with('*') {
        let head = parts[0];
        if !rest.starts_with(head) {
            return false;
        }
        rest = &rest[head.len()..];
    }
    let ends_with_star = pattern.ends_with('*');
    if !ends_with_star {
        let tail = *parts.last().unwrap_or(&"");
        if !rest.ends_with(tail) {
            return false;
        }
        rest = &rest[..rest.len().saturating_sub(tail.len())];
    }
    let inner_end = if ends_with_star {
        parts.len()
    } else {
        parts.len().saturating_sub(1)
    };
    let inner = &parts[1..inner_end];
    for part in inner {
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(idx) => rest = &rest[idx + part.len()..],
            None => return false,
        }
    }
    true
}

/// Catalog entry for discovery (short; not a full schema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    /// What `call_tool` resolves to: the tool's output schema, or `None` for
    /// the `{"content": <text>}` fallback.
    pub returns: Option<serde_json::Value>,
}

/// Session exposure controller: policy + promotions + catalog index.
#[derive(Debug, Default)]
pub struct ExposureController {
    inner: Mutex<ExposureInner>,
}

#[derive(Debug, Default)]
struct ExposureInner {
    policy: ExposurePolicy,
    /// Deferred tools promoted into the active direct set for subsequent model turns.
    promoted: BTreeSet<String>,
    catalog: BTreeMap<String, ToolCatalogEntry>,
    /// Pi `codemode.mode`. `Only` hides policy-`direct` tools (except the
    /// orchestration tools) so the model composes through `codemode`.
    mode: crate::config::CodemodeMode,
}

/// Model-only orchestration tools: always declared, never hidden by `only`.
fn is_orchestration_tool(name: &str) -> bool {
    name == super::CODEMODE_TOOL_NAME || name == super::TOOL_SEARCH_NAME
}

impl ExposureController {
    pub fn new(policy: ExposurePolicy) -> Self {
        Self {
            inner: Mutex::new(ExposureInner {
                policy,
                promoted: BTreeSet::new(),
                catalog: BTreeMap::new(),
                mode: crate::config::CodemodeMode::default(),
            }),
        }
    }

    pub fn with_default_policy() -> Self {
        Self::new(ExposurePolicy::new())
    }

    /// Test helper: index a bare name with no output schema.
    #[cfg(test)]
    pub fn index_tool(&self, name: impl Into<String>, description: impl Into<String>) {
        let name = name.into();
        let entry = ToolCatalogEntry {
            name: name.clone(),
            description: description.into(),
            returns: None,
        };
        self.inner
            .lock()
            .expect("exposure")
            .catalog
            .insert(name, entry);
    }

    #[allow(dead_code)]
    pub fn index_tools(&self, tools: &[Arc<dyn Tool>]) {
        let mut inner = self.inner.lock().expect("exposure");
        for tool in tools {
            let spec = tool.spec();
            inner.catalog.insert(
                spec.name.clone(),
                ToolCatalogEntry {
                    name: spec.name,
                    description: spec.description,
                    returns: tool.output_schema(),
                },
            );
        }
    }

    pub fn reindex_all(&self, tools: &[Arc<dyn Tool>]) {
        let mut inner = self.inner.lock().expect("exposure");
        inner.catalog.clear();
        for tool in tools {
            let spec = tool.spec();
            inner.catalog.insert(
                spec.name.clone(),
                ToolCatalogEntry {
                    name: spec.name,
                    description: spec.description,
                    returns: tool.output_schema(),
                },
            );
        }
    }

    #[allow(dead_code)]
    pub fn set_policy(&self, policy: ExposurePolicy) {
        self.inner.lock().expect("exposure").policy = policy;
    }

    pub fn policy_resolve(&self, name: &str) -> ToolExposure {
        self.inner.lock().expect("exposure").policy.resolve(name)
    }

    /// Effective exposure including promotions (`deferred` → behaves as `direct` for model list).
    pub fn effective(&self, name: &str) -> ToolExposure {
        let inner = self.inner.lock().expect("exposure");
        effective_locked(&inner, name)
    }

    /// Single advertisement predicate for prompt specs and provider requests.
    ///
    /// Matches Pi: `on` declares every `direct` tool next to `codemode`;
    /// `only` leaves policy-`direct` tools out of requests (scripts still call
    /// them). Promoted `deferred` tools stay declared in both modes, as in Pi,
    /// since their exposure is `deferred`, not `direct`.
    pub fn is_model_facing(&self, name: &str) -> bool {
        let inner = self.inner.lock().expect("exposure");
        if effective_locked(&inner, name) != ToolExposure::Direct {
            return false;
        }
        match inner.mode {
            crate::config::CodemodeMode::On => true,
            crate::config::CodemodeMode::Only => {
                is_orchestration_tool(name) || inner.policy.resolve(name) != ToolExposure::Direct
            }
        }
    }

    pub fn mode(&self) -> crate::config::CodemodeMode {
        self.inner.lock().expect("exposure").mode
    }

    /// Sets Pi's `codemode.mode`; takes effect on the next model request.
    pub fn set_mode(&self, mode: crate::config::CodemodeMode) {
        self.inner.lock().expect("exposure").mode = mode;
    }

    /// Scripts may call anything except `hidden`.
    pub fn is_script_callable(&self, name: &str) -> bool {
        self.policy_resolve(name) != ToolExposure::Hidden
    }

    pub fn promote(&self, name: &str) -> bool {
        let mut inner = self.inner.lock().expect("exposure");
        if inner.policy.resolve(name) != ToolExposure::Deferred {
            return false;
        }
        inner.promoted.insert(name.to_owned())
    }

    #[allow(dead_code)]
    pub fn promoted(&self) -> BTreeSet<String> {
        self.inner.lock().expect("exposure").promoted.clone()
    }

    /// Keyword/substring search over name+description (case-insensitive).
    ///
    /// Hidden tools never appear. Used by both `tool_search` and script `search_tools`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<ToolCatalogEntry> {
        let q = query.trim().to_ascii_lowercase();
        let inner = self.inner.lock().expect("exposure");
        let mut hits: Vec<ToolCatalogEntry> = inner
            .catalog
            .values()
            .filter(|entry| {
                if matches!(inner.policy.resolve(&entry.name), ToolExposure::Hidden) {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                entry.name.to_ascii_lowercase().contains(&q)
                    || entry.description.to_ascii_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        hits.sort_by(|a, b| a.name.cmp(&b.name));
        hits.truncate(limit.max(1));
        hits
    }

    pub fn list_script_visible(&self, limit: usize) -> Vec<ToolCatalogEntry> {
        let inner = self.inner.lock().expect("exposure");
        let mut hits: Vec<ToolCatalogEntry> = inner
            .catalog
            .values()
            .filter(|entry| !matches!(inner.policy.resolve(&entry.name), ToolExposure::Hidden))
            .cloned()
            .collect();
        hits.sort_by(|a, b| a.name.cmp(&b.name));
        hits.truncate(limit.max(1));
        hits
    }

    pub fn filter_model_specs(&self, tools: &[Arc<dyn Tool>]) -> Vec<ToolSpec> {
        tools
            .iter()
            .filter(|tool| self.is_model_facing(&tool.spec().name))
            .map(|tool| tool.spec())
            .collect()
    }
}

/// Advertises exactly the model-facing set: `direct` plus promoted `deferred`.
/// `codemode`-only and `hidden` tools stay registered but are never sent to the
/// provider, so MCP schemas stay out of context until promoted.
impl rho_sdk::tool::ToolVisibility for ExposureController {
    fn is_advertised(&self, name: &str) -> bool {
        self.is_model_facing(name)
    }

    /// Pi's `on`-mode hint: a declared tool also says how scripts call it and
    /// what that resolves to. Orchestration tools and `only` mode (where
    /// natives are not declared) are left unchanged.
    fn describe(&self, spec: &mut ToolSpec) {
        let inner = self.inner.lock().expect("exposure");
        if inner.mode != crate::config::CodemodeMode::On || is_orchestration_tool(&spec.name) {
            return;
        }
        let Some(entry) = inner.catalog.get(&spec.name) else {
            return;
        };
        let line = script_call_line(&spec.name, entry.returns.as_ref());
        spec.description = format!("{}\n\n{line}", spec.description.trim_end());
    }
}

/// One line telling the model how a script reaches `name` and what it gets.
fn script_call_line(name: &str, returns: Option<&serde_json::Value>) -> String {
    format!(
        "Codemode: `call_tool(\"{name}\", args)` returns {}.",
        describe_returns(returns)
    )
}

/// Compact shape of a tool's script result, after Pi's `describeOutput`:
/// object fields (optional ones marked `?`), otherwise the schema type.
fn describe_returns(returns: Option<&serde_json::Value>) -> String {
    let Some(schema) = returns else {
        return "`{ content }`".into();
    };
    if let Some(properties) = schema.get("properties").and_then(|value| value.as_object()) {
        let required = schema
            .get("required")
            .and_then(|value| value.as_array())
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let fields = properties
            .keys()
            .map(|field| {
                if required.contains(&field.as_str()) {
                    field.clone()
                } else {
                    format!("{field}?")
                }
            })
            .collect::<Vec<_>>();
        return format!("`{{ {} }}`", fields.join(", "));
    }
    match schema.get("type").and_then(|value| value.as_str()) {
        Some(kind) => format!("`{kind}`"),
        None => "a JSON value".into(),
    }
}

fn effective_locked(inner: &ExposureInner, name: &str) -> ToolExposure {
    let base = inner.policy.resolve(name);
    if base == ToolExposure::Deferred && inner.promoted.contains(name) {
        ToolExposure::Direct
    } else {
        base
    }
}

/// One-line-per-server MCP catalog for the system prompt (not full schemas).
pub fn format_mcp_servers_catalog<'a>(
    servers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let mut lines: Vec<String> = servers
        .into_iter()
        .map(|(identity, status)| format!("- {identity}: {status}"))
        .collect();
    lines.sort();
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\n# MCP servers\n\n");
    out.push_str(
        "Connected MCP servers (tools default to codemode; use tool_search / codemode scripts to discover):\n",
    );
    out.push_str(&lines.join("\n"));
    out.push('\n');
    out
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
