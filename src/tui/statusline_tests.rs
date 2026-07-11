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
        usage: Some(usage.clone()),
        latest_usage: Some(usage),
        context_usage: None,
        provider: "openai".into(),
        model: "gpt-test".into(),
        reasoning: ReasoningLevel::Low,
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
fn statusline_shows_active_goal_indicator() {
    let state = test_state(ModelUsage::default());
    let goal = GoalStatus {
        turns: 2,
        elapsed: Duration::from_secs(65),
    };

    let lines = statusline_lines(&state, 80, Some(&goal));
    let text = line_text(&lines[0]);

    assert!(text.contains("◎ /goal active • 2 turns • 1m 5s"), "{text}");
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

fn test_info(cwd: PathBuf) -> TuiInfo {
    use crate::{
        app::config_repository::ConfigRepository, herdr::HerdrReporter, keybindings::Keybindings,
    };

    TuiInfo {
        cwd,
        provider: "openai".into(),
        model: "gpt-test".into(),
        reasoning: ReasoningLevel::Low,
        show_reasoning_output: true,
        auth: "api-key".into(),
        title_provider: None,
        title_model: None,
        title_auth: None,
        favorite_models: Vec::new(),
        max_tool_output_lines: 10,
        keybindings: Keybindings::default(),
        questionnaire_enabled: true,
        session_id: None,
        recovered_messages: Vec::new(),
        prompt_templates: Default::default(),
        open_resume_picker: false,
        config_repository: ConfigRepository::new(None),
        auth_unavailable: None,
        update_notice: None,
        herdr: HerdrReporter::default(),
    }
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
