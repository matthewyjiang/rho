use super::*;
use pretty_assertions::assert_eq;

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
fn model_segment_prefixes_provider_display_name() {
    assert_eq!(model_segment("OpenAI", "gpt-5.5"), "OpenAI · gpt-5.5");
    assert_eq!(model_segment("", "gpt-5.5"), "gpt-5.5");
    assert_eq!(model_segment("", ""), "");
}

fn span_style(line: &Line<'_>, needle: &str) -> Option<ratatui::style::Style> {
    line.spans
        .iter()
        .find_map(|span| (span.content.as_ref() == needle).then_some(span.style))
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
fn permission_style_marks_auto_as_warning() {
    // Covers: Auto (no checks) must not render like safer permission modes
    // Owner: statusline severity policy
    assert_eq!(permission_style(PermissionMode::Auto), Theme::warning());
    assert_eq!(permission_style(PermissionMode::Plan), Theme::dim());
    assert_eq!(permission_style(PermissionMode::Supervised), Theme::dim());
}

#[test]
fn auto_permission_and_high_context_use_warning_styles() {
    // Covers: painted spans carry severity, not only plain text
    // Owner: statusline render
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.update_usage(None, Some(&ContextUsage::estimated(9_500, Some(10_000))), 0);

    let line = statusline.lines(80, None)[1].clone();
    assert_eq!(
        span_style(&line, "Auto"),
        Some(Theme::warning()),
        "Auto permission must warn: {line:?}"
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
fn provider_degrades_before_model_on_narrow_width() {
    // Covers: adding the provider label must not hide the model on narrow terminals
    // Owner: statusline fit logic
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    // Wide enough for provider+model but not reasoning, then narrow enough that
    // provider must drop while the bare model still fits.
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
fn bottom_row_drops_optional_fields_in_rank_order() {
    // Covers: narrow widths must drop rate before cost before context before model
    // Owner: statusline field hierarchy
    let mut statusline = StatusLine::new(&test_info(PathBuf::from("/tmp/project")));
    statusline.update_usage(None, Some(&ContextUsage::estimated(1_000, Some(10_000))), 0);
    statusline.update_average_output_rate(Some(42));
    // Subagent cost alone is enough to surface a left-side cost field.
    statusline.update_usage(
        None,
        Some(&ContextUsage::estimated(1_000, Some(10_000))),
        12_500,
    );

    let wide = line_text(&statusline.lines(100, None)[1]);
    assert!(
        wide.contains("1.0K (10.0%)")
            && wide.contains("$0.013")
            && wide.contains("42 tok/s avg")
            && wide.contains("OpenAI")
            && wide.contains("gpt-5.5"),
        "wide row should keep ranked fields: {wide:?}"
    );

    let no_rate = line_text(&statusline.lines(56, None)[1]);
    assert!(
        no_rate.contains("1.0K (10.0%)") && no_rate.contains("$0.013"),
        "mid width should keep context and cost: {no_rate:?}"
    );
    assert!(
        !no_rate.contains("tok/s"),
        "rate drops before cost/context: {no_rate:?}"
    );

    let no_cost = line_text(&statusline.lines(32, None)[1]);
    assert!(
        no_cost.contains("1.0K (10.0%)") && no_cost.contains("gpt-5.5"),
        "narrower width should keep context and model: {no_cost:?}"
    );
    assert!(
        !no_cost.contains('$') && !no_cost.contains("OpenAI"),
        "cost and provider drop before model: {no_cost:?}"
    );

    let bare = line_text(&statusline.lines(12, None)[1]);
    assert!(
        bare.contains("Auto"),
        "permission is the last kept field: {bare:?}"
    );
    assert!(
        !bare.contains("gpt-5.5") && !bare.contains("1.0K"),
        "model and context drop before permission: {bare:?}"
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
