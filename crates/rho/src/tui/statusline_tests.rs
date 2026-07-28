use std::fs;

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
