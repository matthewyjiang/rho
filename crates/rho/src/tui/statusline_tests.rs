use super::*;
use pretty_assertions::assert_eq;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn span_style(line: &Line<'_>, needle: &str) -> Option<ratatui::style::Style> {
    line.spans
        .iter()
        .find_map(|span| (span.content.as_ref() == needle).then_some(span.style))
}

fn test_info(cwd: PathBuf) -> RuntimeModelView {
    let mut info = crate::tui::tests::test_bootstrap().runtime;
    info.cwd = cwd;
    info
}

fn fully_populated_statusline() -> StatusLine {
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    // Pin hierarchy inputs so pack tests do not depend on models.dev metadata.
    statusline.state.reasoning_configurable = true;
    statusline.state.reasoning = ReasoningLevel::Medium;
    statusline.state.zen_mode = true;
    statusline.update_usage(
        None,
        Some(&ContextUsage::estimated(1_000, Some(10_000))),
        12_500,
    );
    statusline.update_average_generation_rate(Some(42));
    statusline
}

#[test]
fn statusline_rows_use_display_width_for_alignment() {
    let left = vec![field(
        FieldKey::Context,
        Side::Left,
        RANK_CONTEXT,
        0,
        "项目",
        Theme::dim(),
    )];
    let right = vec![field(
        FieldKey::Model,
        Side::Right,
        RANK_MODEL,
        0,
        "模型",
        Theme::dim(),
    )];
    let line = render_status_row(left, right, 10);
    assert_eq!(display_width(&line_text(&line)), 10);
}

#[test]
fn context_usage_style_escalates_with_fill() {
    // Covers: high context fill must leave ambient dim chrome
    // Owner: statusline severity policy
    assert_eq!(context_usage_style(0.0), Theme::dim());
    assert_eq!(context_usage_style(74.9), Theme::dim());
    assert_eq!(context_usage_style(75.0), Theme::warning());
    assert_eq!(context_usage_style(89.9), Theme::warning());
    assert_eq!(context_usage_style(90.0), Theme::error());
    assert_eq!(context_usage_style(100.0), Theme::error());
}

#[test]
fn permission_style_marks_bypass_as_warning() {
    // Covers: Bypass (no checks) must not render like checked permission modes
    // Owner: statusline severity policy
    assert_eq!(permission_style(PermissionMode::Bypass), Theme::warning());
    assert_eq!(permission_style(PermissionMode::Auto), Theme::dim());
    assert_eq!(permission_style(PermissionMode::Plan), Theme::dim());
    assert_eq!(permission_style(PermissionMode::Supervised), Theme::dim());
}

#[test]
fn bypass_permission_and_high_context_use_warning_styles() {
    // Covers: painted spans carry severity, not only plain text
    // Owner: statusline render
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.state.permission_mode = PermissionMode::Bypass;
    statusline.update_usage(None, Some(&ContextUsage::estimated(9_500, Some(10_000))), 0);

    let line = statusline.lines(80, None)[1].clone();
    assert_eq!(
        span_style(&line, "Bypass"),
        Some(Theme::warning()),
        "Bypass permission must warn: {line:?}"
    );
    assert_eq!(
        span_style(&line, "9.5K (95.0%)"),
        Some(Theme::error()),
        "critical context fill must error: {line:?}"
    );
    assert_eq!(
        span_style(&line, "gpt-5.5"),
        Some(Theme::dim()),
        "model stays ambient: {line:?}"
    );
}

#[test]
fn context_without_known_window_still_shows_tokens() {
    // Covers: models with no reported limit must still show consumption
    // Owner: statusline render
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.update_usage(None, Some(&ContextUsage::estimated(1_234, None)), 0);

    let line = statusline.lines(80, None)[1].clone();
    assert_eq!(
        span_style(&line, "1.2K"),
        Some(Theme::dim()),
        "token count must render dim without a fill percent: {line:?}"
    );
}

#[test]
fn bottom_row_drops_fields_by_global_rank() {
    // Covers: scarce width drops by rank across both sides, not by staged packing
    // Owner: statusline field hierarchy
    let statusline = fully_populated_statusline();
    let state = &statusline.state;

    assert_eq!(
        packed_keys(state, 100),
        vec![
            FieldKey::Context,
            FieldKey::Cost,
            FieldKey::Rate,
            FieldKey::Permission,
            FieldKey::Zen,
            FieldKey::Provider,
            FieldKey::Model,
            FieldKey::Reasoning,
        ],
        "wide row keeps every present field"
    );

    // reasoning drops first
    let after_reasoning = packed_keys(state, 66);
    assert!(
        !after_reasoning.contains(&FieldKey::Reasoning),
        "reasoning is the first drop: {after_reasoning:?}"
    );
    assert!(
        after_reasoning.contains(&FieldKey::Rate),
        "rate outranks reasoning: {after_reasoning:?}"
    );

    // rate before zen
    let after_rate = packed_keys(state, 51);
    assert!(
        !after_rate.contains(&FieldKey::Rate) && !after_rate.contains(&FieldKey::Reasoning),
        "rate drops next: {after_rate:?}"
    );
    assert!(
        after_rate.contains(&FieldKey::Zen),
        "zen outranks rate: {after_rate:?}"
    );

    // zen before provider
    let after_zen = packed_keys(state, 45);
    assert!(
        !after_zen.contains(&FieldKey::Zen),
        "zen drops before provider: {after_zen:?}"
    );
    assert!(
        after_zen.contains(&FieldKey::Provider),
        "provider outranks zen: {after_zen:?}"
    );

    // provider before cost
    let after_provider = packed_keys(state, 36);
    assert!(
        !after_provider.contains(&FieldKey::Provider),
        "provider drops before cost: {after_provider:?}"
    );
    assert!(
        after_provider.contains(&FieldKey::Cost),
        "cost outranks provider: {after_provider:?}"
    );

    // cost before context
    let after_cost = packed_keys(state, 27);
    assert!(
        !after_cost.contains(&FieldKey::Cost),
        "cost drops before context: {after_cost:?}"
    );
    assert!(
        after_cost.contains(&FieldKey::Context) && after_cost.contains(&FieldKey::Model),
        "context and model remain: {after_cost:?}"
    );

    // context before model (cross-side rank that staged packing inverted)
    let after_context = packed_keys(state, 20);
    assert_eq!(
        after_context,
        vec![FieldKey::Permission, FieldKey::Model],
        "context drops before model: {after_context:?}"
    );

    // model before permission
    assert_eq!(
        packed_keys(state, 12),
        vec![FieldKey::Permission],
        "permission is kept last"
    );
}

#[test]
fn pack_prefers_model_over_cost_and_context() {
    // Covers: left metrics must not crowd out the model under the claimed hierarchy
    // Owner: statusline field hierarchy
    let statusline = fully_populated_statusline();

    // Width fits permission + model + cost, but not also context-or-provider noise.
    // cost+perm+model = 6+1+4+3+7 = 21. Force a width where cost and model fight:
    // perm+model = 14, cost+perm = 11, cost+perm+model = 21.
    // At width 15 only perm+model should survive (cost rank 4 < model rank 6).
    assert_eq!(
        packed_keys(&statusline.state, 15),
        vec![FieldKey::Permission, FieldKey::Model]
    );

    // At width 17, context+perm = 17 but context must yield to model.
    assert_eq!(
        packed_keys(&statusline.state, 17),
        vec![FieldKey::Permission, FieldKey::Model]
    );
}

#[test]
fn provider_degrades_before_model_on_narrow_width() {
    // Covers: adding the provider label must not hide the model on narrow terminals
    // Owner: statusline fit logic
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.state.reasoning_configurable = false;

    let wide = statusline.lines(40, None)[1].clone();
    assert!(
        line_text(&wide).contains("OpenAI · gpt-5.5"),
        "wide row should keep provider: {:?}",
        line_text(&wide)
    );

    let narrow = statusline.lines(18, None)[1].clone();
    assert!(
        line_text(&narrow).contains("gpt-5.5"),
        "narrow row should keep the model: {:?}",
        line_text(&narrow)
    );
    assert!(
        !line_text(&narrow).contains("OpenAI"),
        "provider should drop before the model: {:?}",
        line_text(&narrow)
    );
}

#[test]
fn signed_out_keeps_not_signed_in_over_permission() {
    // Covers: signed-out row must still name the auth gap when space is tight
    // Owner: statusline field hierarchy
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.update_signed_in(false);

    assert_eq!(
        packed_keys(&statusline.state, 40),
        vec![
            FieldKey::Permission,
            FieldKey::SignedOut,
            FieldKey::LoginHint,
        ]
    );
    assert_eq!(
        packed_keys(&statusline.state, 20),
        vec![FieldKey::Permission, FieldKey::SignedOut]
    );
    assert_eq!(
        packed_keys(&statusline.state, 10),
        vec![FieldKey::SignedOut],
        "signed-out copy outranks permission"
    );
}

#[test]
fn permission_mode_update_invalidates_cache() {
    let mut info = test_info(PathBuf::from("/tmp/project"));
    let mut statusline = StatusLine::new(&info);
    statusline.lines(18, None);
    let initial_render_count = statusline.render_count();

    info.permission_mode = crate::permission::PermissionMode::Plan;
    statusline.update_model(&info);
    let _ = statusline.lines(18, None);

    assert_eq!(statusline.render_count(), initial_render_count + 1);
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
    use std::fs;

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
    assert_ne!(refreshed, initial);
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
fn fit_cwd_width_zero_is_empty() {
    assert_eq!(fit_cwd("~/work/api-gateway", Some("main"), 0), "");
    assert_eq!(fit_cwd("~/work/api-gateway", None, 0), "");
}

#[test]
fn fit_cwd_drops_branch_when_suffix_fills_width() {
    // " (main)" is wider than 4, so the branch must drop entirely.
    let fitted = fit_cwd("~/work/company/services/api-gateway", Some("main"), 4);
    assert!(!fitted.contains('('), "{fitted}");
    assert!(display_width(&fitted) <= 4, "{fitted}");
}

#[test]
fn fit_cwd_keeps_branch_while_basename_fits() {
    let fitted = fit_cwd("~/work/company/services/api-gateway", Some("main"), 28);
    assert_eq!(fitted, "~/…/api-gateway (main)");
    assert!(display_width(&fitted) <= 28, "{fitted}");
}

#[test]
fn fit_cwd_drops_branch_before_mangling_basename() {
    let path = "~/work/company/services/api-gateway";
    let fitted = fit_cwd(path, Some("very-long-feature-branch"), 18);
    assert!(
        fitted.contains("api-gateway"),
        "basename must remain intact: {fitted}"
    );
    assert!(
        !fitted.contains('('),
        "branch should drop before basename is mangled: {fitted}"
    );
    assert!(display_width(&fitted) <= 18, "{fitted}");
}

#[test]
fn fit_cwd_handles_branch_names_with_parentheses() {
    let fitted = fit_cwd("/tmp/project", Some("feat (wip)"), 40);
    assert_eq!(fitted, "/tmp/project (feat (wip))");

    let fitted = fit_cwd(
        "~/work/company/services/api-gateway",
        Some("feat (wip)"),
        30,
    );
    assert!(fitted.contains("api-gateway"), "{fitted}");
    assert!(
        fitted.ends_with(" (feat (wip))") || !fitted.contains('('),
        "must not re-parse branch from the joined string: {fitted}"
    );
    assert!(display_width(&fitted) <= 30, "{fitted}");
}
