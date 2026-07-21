use super::*;

fn test_info() -> RuntimeInfo {
    RuntimeInfo {
        version: "1.9.0".into(),
        provider: "openai".into(),
        model: "gpt-test".into(),
        reasoning: "medium".into(),
        permission_mode: "auto".into(),
        billing: BillingInfo::Metered,
        cwd: PathBuf::from("/tmp/project"),
        branch: Some("main".into()),
        usage: Some(ModelUsage {
            input_tokens: Some(300_000),
            output_tokens: Some(100_000),
            cache_read_tokens: Some(700_000),
            cache_write_tokens: Some(25_000),
            cost_usd_micros: Some(1_250_000),
            ..ModelUsage::default()
        }),
        latest_usage: Some(ModelUsage {
            input_tokens: Some(100_000),
            cache_read_tokens: Some(900_000),
            ..ModelUsage::default()
        }),
        context_usage: Some(ContextUsage::estimated(25_000, Some(100_000))),
        model_metadata: None,
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn rendered_text(info: &RuntimeInfo, width: usize) -> String {
    runtime_info_lines(info, width)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn runtime_info_groups_model_usage_and_workspace_details() {
    let text = rendered_text(&test_info(), 80);

    assert!(text.contains("rho  v1.9.0"), "{text}");
    assert!(text.contains("Model\n"), "{text}");
    assert!(text.contains("Provider      openai"), "{text}");
    assert!(text.contains("Session usage\n"), "{text}");
    assert!(
        text.contains("25,000 / 100,000 tokens (25.0%, estimated)"),
        "{text}"
    );
    assert!(text.contains("Input tokens  300,000"), "{text}");
    assert!(text.contains("Cache read    700,000"), "{text}");
    assert!(
        text.contains("Cache hit     90.0% on the latest request"),
        "{text}"
    );
    assert!(text.contains("Cost          $1.250"), "{text}");
    assert!(text.contains("Workspace\n"), "{text}");
    assert!(text.contains("Git branch    main"), "{text}");
}

#[test]
fn narrow_runtime_info_stacks_labels_and_values() {
    let lines = runtime_info_lines(&test_info(), 24);
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(lines.iter().all(|line| line.width() <= 24));
    assert!(text.contains("  Provider\n    openai"), "{text}");
    assert!(text.contains("  Input tokens\n    300,000"), "{text}");
}

#[test]
fn runtime_info_wraps_long_values_without_losing_details() {
    let lines = runtime_info_lines(&test_info(), 40);
    let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(lines.iter().all(|line| line.width() <= 40));
    assert!(text.contains("25.0%, estimated"), "{text}");
    assert!(!text.contains('…'), "{text}");
}

#[test]
fn format_number_adds_thousands_separators() {
    assert_eq!(format_number(12), "12");
    assert_eq!(format_number(1_234), "1,234");
    assert_eq!(format_number(12_345_678), "12,345,678");
}
