use std::fs;

use super::*;
use rho_providers::model::models_dev::ModelCost;

fn priced_metadata() -> ModelMetadata {
    ModelMetadata {
        cost_default: Some(ModelCost {
            input_micros_per_m: Some(1_000_000),
            output_micros_per_m: Some(2_000_000),
            cache_read_micros_per_m: Some(100_000),
            cache_write_micros_per_m: None,
        }),
        advertised_context_window: Some(100_000),
        ..ModelMetadata::default()
    }
}

fn test_state(usage: ModelUsage) -> StatusLineState {
    StatusLineState {
        cwd: PathBuf::from("/tmp/project"),
        branch: None,
        usage: Some(usage),
        context_usage: Some(ContextUsage::estimated(25_000, Some(100_000))),
        provider: "openai".into(),
        model: "gpt-test".into(),
        reasoning: ReasoningLevel::Low,
        reasoning_configurable: true,
        permission_mode: crate::permission::PermissionMode::Auto,
        model_metadata: Some(priced_metadata()),
        subagent_total_cost_usd_micros: 0,
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
    assert_eq!(display_width(&line_text(&line)), 10);
}

#[test]
fn wide_statusline_keeps_only_summary_fields() {
    let usage = ModelUsage {
        input_tokens: Some(300_000),
        output_tokens: Some(100_000),
        cache_read_tokens: Some(700_000),
        cache_write_tokens: Some(25_000),
        cost_usd_micros: Some(570_000),
        ..ModelUsage::default()
    };

    let lines = statusline_lines(&test_state(usage), 80, None);
    let bottom = line_text(&lines[1]);

    assert!(bottom.contains("25.0K (25.0%)"), "{bottom}");
    assert!(bottom.contains("$0.570"), "{bottom}");
    assert!(bottom.contains("Auto · gpt-test · low"), "{bottom}");
    assert!(!bottom.contains("300.0k"), "{bottom}");
    assert!(!bottom.contains("CH"), "{bottom}");
    assert!(!bottom.contains("openai"), "{bottom}");
}

#[test]
fn statusline_includes_subagent_cost_in_total() {
    let usage = ModelUsage {
        cost_usd_micros: Some(570_000),
        ..ModelUsage::default()
    };
    let mut state = test_state(usage);
    state.subagent_total_cost_usd_micros = 430_000;

    let bottom = line_text(&statusline_lines(&state, 80, None)[1]);

    assert!(bottom.contains("$1.000"), "{bottom}");
    assert!(!bottom.contains("$0.570"), "{bottom}");
}

#[test]
fn statusline_can_show_subagent_cost_without_main_usage_cost() {
    let mut state = test_state(ModelUsage::default());
    state.usage = Some(ModelUsage::default());
    state.subagent_total_cost_usd_micros = 250_000;

    let bottom = line_text(&statusline_lines(&state, 80, None)[1]);

    assert!(bottom.contains("$0.250"), "{bottom}");
}

#[test]
fn narrow_statusline_drops_whole_optional_fields() {
    let usage = ModelUsage {
        cost_usd_micros: Some(570_000),
        ..ModelUsage::default()
    };
    let lines = statusline_lines(&test_state(usage), 24, None);
    let bottom = line_text(&lines[1]);

    assert!(bottom.contains("25.0K (25.0%)"), "{bottom}");
    assert!(bottom.contains("Auto"), "{bottom}");
    assert!(!bottom.contains('$'), "{bottom}");
    assert!(!bottom.contains("low"), "{bottom}");
    assert!(!bottom.contains("gpt-test"), "{bottom}");
    assert!(!bottom.contains('…'), "{bottom}");
    assert!(display_width(&bottom) <= 24);
}

#[test]
fn very_narrow_statusline_drops_context_to_preserve_permission_mode() {
    let mut state = test_state(ModelUsage::default());
    state.permission_mode = crate::permission::PermissionMode::Supervised;

    let bottom = line_text(&statusline_lines(&state, 12, None)[1]);

    assert!(bottom.contains("Supervised"), "{bottom}");
    assert!(!bottom.contains('%'), "{bottom}");
    assert!(!bottom.contains('K'), "{bottom}");
}

#[test]
fn statusline_omits_reasoning_when_it_is_not_configurable() {
    let mut state = test_state(ModelUsage::default());
    state.provider = "github-copilot".into();
    state.reasoning_configurable = false;

    let bottom = line_text(&statusline_lines(&state, 80, None)[1]);

    assert!(bottom.contains("Auto · gpt-test"), "{bottom}");
    assert!(!bottom.contains("low"), "{bottom}");
}

#[test]
fn statusline_shows_active_goal_indicator() {
    let goal = GoalStatus {
        turns: 2,
        elapsed: Duration::from_secs(65),
        blocked: false,
    };

    let text = line_text(&statusline_lines(&test_state(ModelUsage::default()), 80, Some(&goal))[0]);

    assert!(text.contains("goal: active • 2 turns • 1m 5s"), "{text}");
}

#[test]
fn statusline_shows_blocked_goal_indicator() {
    let goal = GoalStatus {
        turns: 1,
        elapsed: Duration::from_secs(9),
        blocked: true,
    };

    let text = line_text(&statusline_lines(&test_state(ModelUsage::default()), 80, Some(&goal))[0]);

    assert!(text.contains("goal: blocked • 1 turn • 9s"), "{text}");
}

#[test]
fn context_summary_formats_tokens_and_percent() {
    assert_eq!(
        format_context_summary(&test_state(ModelUsage::default())),
        "25.0K (25.0%)"
    );
}

fn test_info(cwd: PathBuf) -> RuntimeModelView {
    let mut info = crate::tui::tests::test_bootstrap().runtime;
    info.cwd = cwd;
    info
}

#[test]
fn permission_mode_update_invalidates_cache() {
    let mut info = test_info(PathBuf::from("/tmp/project"));
    let mut statusline = StatusLine::new(&info);
    statusline.lines(18, None);
    let initial_render_count = statusline.render_count();

    info.permission_mode = crate::permission::PermissionMode::Plan;
    statusline.update_model(&info);
    let lines = statusline.lines(18, None).to_vec();

    assert_eq!(statusline.render_count(), initial_render_count + 1);
    assert!(line_text(&lines[1]).contains("Plan"));
}

#[test]
fn unchanged_statusline_reuses_rendered_lines() {
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.lines(80, None);
    statusline.lines(80, None);

    assert_eq!(statusline.render_count(), 1);
}

#[test]
fn git_branch_is_cached_until_explicit_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let git_dir = temp.path().join(".git");
    fs::create_dir(&git_dir).unwrap();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let mut statusline = StatusLine::new(&test_info(temp.path().to_path_buf()));

    let initial = statusline.lines(80, None).to_vec();
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature\n").unwrap();
    let cached = statusline.lines(80, None).to_vec();
    statusline.refresh_git_branch();
    let refreshed = statusline.lines(80, None).to_vec();

    assert_eq!(cached, initial);
    assert!(line_text(&initial[0]).contains("(main)"));
    assert!(line_text(&refreshed[0]).contains("(feature)"));
}

#[test]
fn shorten_path_keeps_trailing_segments() {
    assert_eq!(
        shorten_path_display("~/work/company/services/api-gateway", 24),
        "~/…/services/api-gateway"
    );
    assert_eq!(
        shorten_path_display("~/work/company/services/api-gateway", 18),
        "~/…/api-gateway"
    );
    assert_eq!(
        shorten_path_display("/tmp/claude-1000/home-emgym-herdr-work", 23),
        "…/home-emgym-herdr-work"
    );
    assert_eq!(
        shorten_path_display("/tmp/claude-1000/projects/api-gateway", 20),
        "/…/api-gateway"
    );
}

#[test]
fn shorten_path_keeps_end_when_last_segment_is_long() {
    let shortened = shorten_path_display("~/work/company/very-long-service-name", 14);
    assert!(shortened.starts_with('…'), "{shortened}");
    assert!(
        shortened.ends_with("service-name") || shortened.ends_with("name"),
        "{shortened}"
    );
    assert!(display_width(&shortened) <= 14, "{shortened}");
    assert!(!shortened.starts_with("~/work"), "{shortened}");
}

#[test]
fn fit_status_cwd_keeps_branch_and_tail() {
    let fitted = fit_status_cwd_left("~/work/company/services/api-gateway (main)", 28);
    assert!(fitted.contains("api-gateway"), "{fitted}");
    assert!(fitted.ends_with(" (main)"), "{fitted}");
    assert!(display_width(&fitted) <= 28, "{fitted}");
    assert!(!fitted.contains("work/company/services"), "{fitted}");
}

#[test]
fn narrow_statusline_keeps_cwd_basename() {
    let mut state = test_state(ModelUsage::default());
    state.cwd = PathBuf::from("/tmp/claude-1000/home-emgym-herdr-worktree-api-gateway");
    state.branch = None;

    let top = line_text(&statusline_lines(&state, 40, None)[0]);
    let cwd = top.trim_end();

    assert!(cwd.contains("api-gateway"), "{cwd}");
    assert!(cwd.contains('…'), "{cwd}");
    assert!(!cwd.ends_with('…'), "{cwd}");
    assert!(display_width(cwd) <= 40, "{cwd}");
}
