use pretty_assertions::assert_eq;

use super::*;

// Covers: only mode must suppress base-direct tools without suppressing promoted
// deferred tools, and promotion must never expose codemode-only or hidden tools.
// Owner: exposure advertisement policy.
#[test]
fn advertisement_uses_base_exposure_promotion_and_mode() {
    use crate::config::CodemodeMode::{On, Only};
    use ToolExposure::{Codemode, Deferred, Direct, Hidden};

    for (base, promoted, on, only) in [
        (Direct, false, true, false),
        (Direct, true, true, false),
        (Codemode, false, false, false),
        (Codemode, true, false, false),
        (Deferred, false, false, false),
        (Deferred, true, true, true),
        (Hidden, false, false, false),
        (Hidden, true, false, false),
    ] {
        for (mode, expected) in [(On, on), (Only, only)] {
            for name in [
                "helper",
                super::super::CODEMODE_TOOL_NAME,
                super::super::TOOL_SEARCH_NAME,
            ] {
                let ctrl =
                    ExposureController::new(ExposurePolicy::new().override_exact(name, Deferred));
                ctrl.set_mode(mode);
                if promoted {
                    ctrl.promote(name);
                }
                // Include retained promotions after a policy change: only the
                // current base exposure decides whether promotion matters.
                ctrl.set_policy(ExposurePolicy::new().override_exact(name, base));
                let orchestration_direct = name != "helper" && base == Direct;
                assert_eq!(
                    ctrl.is_model_facing(name),
                    expected || orchestration_direct,
                    "name={name} base={base:?} promoted={promoted} mode={mode:?}"
                );
            }
        }
    }
}

#[test]
fn default_mcp_is_codemode_natives_direct() {
    let policy = ExposurePolicy::new();
    assert_eq!(policy.resolve("bash"), ToolExposure::Direct);
    assert_eq!(policy.resolve("read_file"), ToolExposure::Direct);
    assert_eq!(
        policy.resolve("mcp__filesystem__read_file"),
        ToolExposure::Codemode
    );
}

#[test]
fn exact_override_beats_pattern_and_default() {
    let policy = ExposurePolicy::new()
        .override_pattern("mcp__computer__*", ToolExposure::Direct)
        .override_exact("mcp__computer__screenshot", ToolExposure::Hidden);
    assert_eq!(policy.resolve("mcp__computer__click"), ToolExposure::Direct);
    assert_eq!(
        policy.resolve("mcp__computer__screenshot"),
        ToolExposure::Hidden
    );
    assert_eq!(policy.resolve("mcp__other__x"), ToolExposure::Codemode);
}

#[test]
fn model_facing_drops_codemode_keeps_direct() {
    let ctrl = ExposureController::with_default_policy();
    ctrl.index_tool("bash", "run shell");
    ctrl.index_tool("mcp__fs__read", "read via mcp");
    ctrl.index_tool("codemode", "starlark");
    assert!(ctrl.is_model_facing("bash"));
    assert!(ctrl.is_model_facing("codemode"));
    assert!(!ctrl.is_model_facing("mcp__fs__read"));
    assert!(ctrl.is_script_callable("mcp__fs__read"));
}

#[test]
fn deferred_promoted_by_tool_search_then_model_facing() {
    let policy = ExposurePolicy::new().override_exact("rare_tool", ToolExposure::Deferred);
    let ctrl = ExposureController::new(policy);
    ctrl.index_tool("rare_tool", "rarely used helper");
    ctrl.index_tool("bash", "shell");

    assert!(!ctrl.is_model_facing("rare_tool"));
    let hits = ctrl.search("rarely", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "rare_tool");
    assert!(ctrl.promote("rare_tool"));
    assert!(ctrl.is_model_facing("rare_tool"));
    assert_eq!(ctrl.effective("rare_tool"), ToolExposure::Direct);
}

#[test]
fn hidden_unreachable_in_search_and_script() {
    let policy = ExposurePolicy::new().override_exact("secret", ToolExposure::Hidden);
    let ctrl = ExposureController::new(policy);
    ctrl.index_tool("secret", "do not find me");
    ctrl.index_tool("bash", "shell");
    assert!(ctrl.search("secret", 10).is_empty());
    assert!(ctrl.search("do not find", 10).is_empty());
    assert!(!ctrl.is_script_callable("secret"));
    assert!(!ctrl.is_model_facing("secret"));
    assert!(!ctrl.promote("secret"));
}

#[test]
fn script_search_finds_mcp_codemode_tools() {
    let ctrl = ExposureController::with_default_policy();
    ctrl.index_tool("mcp__github__create_issue", "Create a GitHub issue");
    ctrl.index_tool("bash", "shell");
    let hits = ctrl.search("github", 10);
    assert!(hits.iter().any(|h| h.name.contains("github")));
    let listed = ctrl.list_script_visible(50);
    assert!(listed.iter().any(|h| h.name.starts_with("mcp__")));
}

#[test]
fn override_to_direct_makes_mcp_model_facing() {
    let policy = ExposurePolicy::new().override_pattern("mcp__computer__*", ToolExposure::Direct);
    let ctrl = ExposureController::new(policy);
    ctrl.index_tool("mcp__computer__click", "click");
    ctrl.index_tool("mcp__fs__read", "read");
    assert!(ctrl.is_model_facing("mcp__computer__click"));
    assert!(!ctrl.is_model_facing("mcp__fs__read"));
}

#[test]
fn mcp_servers_catalog_is_short() {
    let text = format_mcp_servers_catalog([("github", "connected"), ("filesystem", "connected")]);
    assert!(text.contains("# MCP servers"));
    assert!(text.contains("- filesystem: connected"));
    assert!(text.contains("- github: connected"));
    assert!(!text.contains("parameters"));
    assert!(!text.contains("input_schema"));
}

#[test]
fn glob_match_basics() {
    assert!(glob_match("mcp__computer__*", "mcp__computer__click"));
    assert!(!glob_match("mcp__computer__*", "mcp__fs__read"));
    assert!(glob_match("*", "anything"));
    assert!(glob_match("exact", "exact"));
    assert!(!glob_match("exact", "other"));
    assert!(glob_match("*__click", "mcp__computer__click"));
}
