use super::*;

// Covers: large edit preview checkpoints grow with buffer size
// Owner: interactive presenter
#[test]
fn edit_preview_stride_grows_with_input_size() {
    let limit = EDIT_STREAM_PREVIEW_LIMIT;
    let min_stride = EDIT_STREAM_PREVIEW_STRIDE;

    assert_eq!(
        ToolKind::Edit.preview_parse_stride(limit),
        limit.max(min_stride)
    );
    assert_eq!(
        ToolKind::Edit.preview_parse_stride(limit * 2),
        (limit * 2).max(min_stride)
    );
    assert_eq!(
        ToolKind::Edit.preview_parse_stride(limit * 3),
        (limit * 3).max(min_stride)
    );

    // Geometric checkpoints: each interval is at least as large as the last.
    let mut size = limit;
    let mut previous_stride = 0usize;
    for _ in 0..4 {
        let stride = ToolKind::Edit.preview_parse_stride(size);
        assert!(stride >= min_stride);
        assert!(stride >= previous_stride);
        previous_stride = stride;
        size = size.saturating_add(stride);
    }
}

// Covers: below the edit limit, cadence matches the generic policy
// Owner: interactive presenter
#[test]
fn edit_preview_uses_generic_cadence_below_stream_limit() {
    assert_eq!(ToolKind::Edit.preview_parse_stride(0), 0);
    assert_eq!(
        ToolKind::Edit.preview_parse_stride(PREVIEW_FULL_PARSE_LIMIT - 1),
        0
    );
    assert_eq!(
        ToolKind::Edit.preview_parse_stride(PREVIEW_FULL_PARSE_LIMIT),
        PREVIEW_LARGE_PARSE_STRIDE
    );
    assert_eq!(
        ToolKind::Edit.preview_parse_stride(EDIT_STREAM_PREVIEW_LIMIT - 1),
        PREVIEW_LARGE_PARSE_STRIDE
    );
}

// Covers: approval cards must flag document-only fallback and keep pure CUT as
// content stats, never file-delete
// Owner: interactive presenter
#[test]
fn edit_planned_card_surfaces_unverified_document_fallback() {
    let dir = tempfile::TempDir::new().unwrap();
    let view = ToolView {
        kind: ToolKind::Edit,
        name: "edit".into(),
        arguments: serde_json::json!({
            "input": "[missing.txt#AAAA]\nCUT 1.=2\n"
        }),
        metadata: Default::default(),
    };
    let card = start_card(&view, dir.path());
    assert!(
        card.facts.iter().any(|fact| matches!(
            fact,
            rho_tools::tool_card::ToolFact::Meta { text }
                if text.contains("document preview only")
        )),
        "expected unverified notice in facts: {:?}",
        card.facts
    );
    assert!(
        card.facts.iter().any(|fact| matches!(
            fact,
            rho_tools::tool_card::ToolFact::DiffStat {
                added: 0,
                removed: 2,
                ..
            }
        )),
        "pure CUT must keep DiffStat, not file-delete: {:?}",
        card.facts
    );
    assert!(
        !card.facts.iter().any(|fact| matches!(
            fact,
            rho_tools::tool_card::ToolFact::Meta { text } if text.starts_with("delete")
        )),
        "must not render file-delete meta for line CUT: {:?}",
        card.facts
    );
}
