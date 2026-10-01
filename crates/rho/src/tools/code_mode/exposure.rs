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

    pub fn override_exact(mut self, name: impl Into<String>, exposure: ToolExposure) -> Self {
        self.overrides
            .push(ExposureOverride::exact(name, exposure));
        self
    }

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
}

impl ExposureController {
    pub fn new(policy: ExposurePolicy) -> Self {
        Self {
            inner: Mutex::new(ExposureInner {
                policy,
                promoted: BTreeSet::new(),
                catalog: BTreeMap::new(),
            }),
        }
    }

    pub fn with_default_policy() -> Self {
        Self::new(ExposurePolicy::new())
    }

    pub fn index_tool(&self, name: impl Into<String>, description: impl Into<String>) {
        let name = name.into();
        let entry = ToolCatalogEntry {
            name: name.clone(),
            description: description.into(),
        };
        self.inner
            .lock()
            .expect("exposure")
            .catalog
            .insert(name, entry);
    }

    #[allow(dead_code)]
    pub fn index_tools(&self, tools: &[Arc<dyn Tool>]) {
        for tool in tools {
            let spec = tool.spec();
            self.index_tool(spec.name, spec.description);
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

    pub fn is_model_facing(&self, name: &str) -> bool {
        self.effective(name) == ToolExposure::Direct
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
