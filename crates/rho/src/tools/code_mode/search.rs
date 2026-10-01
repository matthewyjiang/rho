//! Tool search / deferred loading hooks for code-mode (ship requirement).
//!
//! # Why this exists
//!
//! Pi 0.99 / Earendil: MCP catalogs must not dump every schema into model
//! context. Composition (`codemode`) needs **discovery** (search / deferred)
//! so the model (or script) can find tools on demand.
//!
//! Dual direct+codemode exposure is an OK **prototype crutch** until this
//! lands. Ship bar = search/deferred + `codemode` composition + ToolHost bridge
//! + nested approvals + registry wiring.
//!
//! # v0 status
//!
//! Types and TODOs only — no live search index yet. Wire against Rho's tool
//! registry / MCP metadata when exposure flags exist.

#![allow(dead_code)]

/// How a tool may appear relative to the model vs `codemode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    /// Listed in the model tool set (direct calls).
    Direct,
    /// Only reachable via `codemode` / `call_tool` (and optionally search).
    CodemodeOnly,
    /// Not listed until discovered via search / deferred load.
    Deferred,
    /// Both direct and codemode (prototype crutch — not the ship default).
    DirectAndCodemode,
}

/// One searchable tool summary (short; not a full JSON schema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchHit {
    pub name: String,
    pub description: String,
    pub exposure: ToolExposure,
}

/// Placeholder search API for deferred discovery.
///
/// TODO: implement against the live tool/MCP registry with pagination and
/// schema-on-demand (`describe(name) -> schema`) so prompts stay small.
pub trait ToolSearch: Send + Sync {
    fn search(&self, query: &str, limit: usize) -> Vec<ToolSearchHit>;
}

/// Empty stub used until registry-backed search exists.
#[derive(Debug, Default)]
pub struct UnimplementedToolSearch;

impl ToolSearch for UnimplementedToolSearch {
    fn search(&self, _query: &str, _limit: usize) -> Vec<ToolSearchHit> {
        Vec::new()
    }
}
