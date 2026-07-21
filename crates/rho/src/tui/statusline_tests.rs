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

    assert!(bottom.contains("~25.0% ctx"), "{bottom}");
    assert!(bottom.contains("$0.570"), "{bottom}");
    assert!(bottom.contains("Auto · gpt-test · low"), "{bottom}");
    assert!(!bottom.contains("300.0k"), "{bottom}");
    assert!(!bottom.contains("CH"), "{bottom}");
    assert!(!bottom.contains("openai"), "{bottom}");
}

#[test]
fn narrow_statusline_drops_whole_optional_fields() {
    let usage = ModelUsage {
        cost_usd_micros: Some(570_000),
        ..ModelUsage::default()
    };
    let lines = statusline_lines(&test_state(usage), 24, None);
    let bottom = line_text(&lines[1]);

    assert!(bottom.contains("~25.0% ctx"), "{bottom}");
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
    assert!(!bottom.contains("ctx"), "{bottom}");
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
fn context_summary_marks_estimates() {
    assert_eq!(
        format_context_summary(&test_state(ModelUsage::default())),
        "~25.0% ctx"
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
