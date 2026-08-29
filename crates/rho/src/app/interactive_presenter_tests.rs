use super::*;

// Covers: every selectable edit schema routes through file-diff presentation
// Owner: interactive presenter
#[test]
fn selected_edit_tool_names_share_the_edit_presentation_kind() {
    assert_eq!(
        ToolKind::from_name("apply_patch"),
        ToolKind::Edit(rho_tools::EditFormat::ApplyPatch)
    );
    assert_eq!(
        ToolKind::from_name("edit"),
        ToolKind::Edit(rho_tools::EditFormat::Hashline)
    );
    assert_eq!(
        ToolKind::from_name("str_replace"),
        ToolKind::Edit(rho_tools::EditFormat::StrReplace)
    );
}

// Covers: started keeps str_replace identity from the unique tool name
// Owner: interactive presenter
#[test]
fn started_keeps_str_replace_kind_from_tool_name() {
    let mut presenter = InteractiveToolPresenter::new(std::path::PathBuf::from("."));
    let call = ToolCall {
        id: "call-1".into(),
        name: "str_replace".into(),
        arguments: serde_json::json!({
            "path": "a.rs",
            "old_string": "a",
            "new_string": "b"
        }),
    };
    let _ = presenter.proposed(call);
    let presented = presenter.started(
        ToolCallId::from_string("call-1").unwrap(),
        "str_replace".into(),
        ToolMetadata::default(),
    );
    match &presented.card.header {
        rho_tools::tool_card::ToolHeader::Call { verb, primary } => {
            assert_eq!(verb, "str_replace");
            assert_eq!(primary.as_deref(), Some("a.rs"));
        }
        other => panic!("expected call header, got {other:?}"),
    }
}

// Covers: large edit preview checkpoints grow with buffer size
// Owner: interactive presenter
#[test]
fn edit_preview_stride_grows_with_input_size() {
    let limit = EDIT_STREAM_PREVIEW_LIMIT;
    let min_stride = EDIT_STREAM_PREVIEW_STRIDE;

    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline).preview_parse_stride(limit),
        limit.max(min_stride)
    );
    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline).preview_parse_stride(limit * 2),
        (limit * 2).max(min_stride)
    );
    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline).preview_parse_stride(limit * 3),
        (limit * 3).max(min_stride)
    );

    // Geometric checkpoints: each interval is at least as large as the last.
    let mut size = limit;
    let mut previous_stride = 0usize;
    for _ in 0..4 {
        let stride = ToolKind::Edit(rho_tools::EditFormat::Hashline).preview_parse_stride(size);
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
    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline).preview_parse_stride(0),
        0
    );
    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline)
            .preview_parse_stride(PREVIEW_FULL_PARSE_LIMIT - 1),
        0
    );
    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline)
            .preview_parse_stride(PREVIEW_FULL_PARSE_LIMIT),
        PREVIEW_LARGE_PARSE_STRIDE
    );
    assert_eq!(
        ToolKind::Edit(rho_tools::EditFormat::Hashline)
            .preview_parse_stride(EDIT_STREAM_PREVIEW_LIMIT - 1),
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
        kind: ToolKind::Edit(rho_tools::EditFormat::Hashline),
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
                if text.as_str() == rho_tools::hashline::EDIT_DOCUMENT_ONLY_NOTICE
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

// Covers: advisor cards keep phase on the header and body as plain guidance
// Owner: interactive presenter
#[test]
fn advisor_cards_use_status_first_headers() {
    use rho_tools::tool_card::{ToolBody, ToolHeader, ToolStatus};

    let dir = tempfile::TempDir::new().unwrap();
    let view = ToolView {
        kind: ToolKind::Advisor,
        name: "advisor".into(),
        arguments: serde_json::json!({}),
        metadata: Default::default(),
    };

    let start = start_card(&view, dir.path());
    assert_eq!(start.status, ToolStatus::Running);
    assert_eq!(
        start.header,
        ToolHeader::status_first(
            "advisor",
            crate::agent::OneShotPhase::WaitingForProvider.label()
        )
    );

    let progress = progress_card(
        Some((&view, dir.path())),
        &rho_sdk::tool::ToolProgress::message("try the simpler path")
            .metadata(rho_sdk::tool::ToolMetadata::new().command_summary("responding")),
    );
    assert_eq!(
        progress.header,
        ToolHeader::status_first("advisor", "responding")
    );
    assert_eq!(
        progress.body,
        ToolBody::Lines(vec!["try the simpler path".into()])
    );

    let progress_fallback = progress_card(
        Some((&view, dir.path())),
        &rho_sdk::tool::ToolProgress::message(""),
    );
    assert_eq!(
        progress_fallback.header,
        ToolHeader::status_first(
            "advisor",
            crate::agent::OneShotPhase::WaitingForProvider.label()
        )
    );

    let interrupted = interrupted_card(&view, "", dir.path());
    assert_eq!(interrupted.status, ToolStatus::Interrupted);
    assert_eq!(
        interrupted.header,
        ToolHeader::status_first("advisor", "interrupted")
    );

    let finished = finished_card(&view, "final guidance", true, dir.path());
    assert_eq!(finished.status, ToolStatus::Ok);
    assert_eq!(
        finished.header,
        ToolHeader::status_first("advisor", "completed")
    );
    assert_eq!(
        finished.body,
        ToolBody::Lines(vec!["final guidance".into()])
    );

    let failed = finished_card(&view, "advisor blew up", false, dir.path());
    assert_eq!(failed.status, ToolStatus::Error);
    assert_eq!(failed.header, ToolHeader::status_first("advisor", "failed"));
}

// Covers: shell start cards must expose the typed timeout budget fact
// Owner: interactive presenter
#[test]
fn shell_start_card_includes_timeout_fact() {
    use rho_tools::tool_card::{ToolFact, ToolHeader, ToolStatus};

    let dir = tempfile::TempDir::new().unwrap();
    let with_timeout = ToolView {
        kind: ToolKind::Bash,
        name: "bash".into(),
        arguments: serde_json::json!({
            "command": "sleep 1",
            "timeout_seconds": 30
        }),
        metadata: Default::default(),
    };
    let card = start_card(&with_timeout, dir.path());
    assert_eq!(card.status, ToolStatus::Running);
    assert_eq!(card.header, ToolHeader::shell("$", Some("sleep 1".into())));
    assert_eq!(card.facts, vec![ToolFact::Timeout { seconds: Some(30) }]);

    let no_timeout = ToolView {
        kind: ToolKind::Bash,
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "true" }),
        metadata: Default::default(),
    };
    let card = start_card(&no_timeout, dir.path());
    assert_eq!(card.facts, vec![ToolFact::Timeout { seconds: None }]);
}

// Covers: MCP start cards decode the tool verb and show server + remaining
// args, not the raw exported name or JSON blob
// Owner: interactive presenter
#[test]
fn mcp_proposed_and_started_cards_use_decoded_verb_and_argument_facts() {
    use rho_tools::tool_card::{ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};

    let mut presenter = InteractiveToolPresenter::new(std::path::PathBuf::from("."));
    let call = ToolCall {
        id: "call-mcp".into(),
        name: "mcp__olive_salmon__increase_grep".into(),
        arguments: serde_json::json!({
            "path": "crates",
            "output_mode": "files_with_matches",
            "max_results": 50
        }),
    };
    let expected = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call("increase_grep", Some("crates".into())),
    )
    .with_facts(vec![
        ToolFact::Meta {
            text: "mcp · olive_salmon".into(),
        },
        ToolFact::Text {
            text: "output_mode files_with_matches · max_results 50".into(),
        },
    ]);

    let proposed = presenter.proposed(call);
    assert_eq!(proposed.card, expected);
    assert_eq!(
        ToolKind::from_name("mcp__olive_salmon__increase_grep"),
        ToolKind::Mcp
    );

    let started = presenter.started(
        ToolCallId::from_string("call-mcp").unwrap(),
        "mcp__olive_salmon__increase_grep".into(),
        ToolMetadata::default(),
    );
    assert_eq!(started.card, expected);
}

// Covers: successful MCP results keep provenance/args and add a line count
// plus the output body
// Owner: interactive presenter
#[test]
fn mcp_finished_ok_card_counts_lines_and_keeps_body() {
    use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};

    let dir = tempfile::TempDir::new().unwrap();
    let view = ToolView {
        kind: ToolKind::Mcp,
        name: "mcp__olive_salmon__increase_grep".into(),
        arguments: serde_json::json!({
            "path": "crates",
            "output_mode": "files_with_matches",
            "max_results": 50
        }),
        metadata: Default::default(),
    };
    let content = "crates/rho/src/lib.rs\ncrates/rho/src/app.rs\n";
    let card = finished_card(&view, content, true, dir.path());
    assert_eq!(
        card,
        ToolCard::new(
            ToolStatus::Ok,
            ToolFamily::Default,
            ToolHeader::call("increase_grep", Some("crates".into())),
        )
        .with_facts(vec![
            ToolFact::Meta {
                text: "mcp · olive_salmon".into(),
            },
            ToolFact::Text {
                text: "output_mode files_with_matches · max_results 50".into(),
            },
            ToolFact::Count {
                label: "lines".into(),
                value: 2,
                detail: None,
            },
        ])
        .with_body(ToolBody::Lines(vec![
            "crates/rho/src/lib.rs".into(),
            "crates/rho/src/app.rs".into(),
        ]))
    );
}

// Covers: failed MCP results keep server provenance and use the shared error fact
// Owner: interactive presenter
#[test]
fn mcp_finished_error_card_keeps_server_fact_and_error_summary() {
    use rho_tools::tool_card::{ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};

    let dir = tempfile::TempDir::new().unwrap();
    let view = ToolView {
        kind: ToolKind::Mcp,
        name: "mcp__olive_salmon__increase_grep".into(),
        arguments: serde_json::json!({ "path": "crates" }),
        metadata: Default::default(),
    };
    let card = finished_card(&view, "server unavailable", false, dir.path());
    assert_eq!(
        card,
        ToolCard::new(
            ToolStatus::Error,
            ToolFamily::Default,
            ToolHeader::call("increase_grep", Some("crates".into())),
        )
        .with_facts(vec![
            ToolFact::Meta {
                text: "mcp · olive_salmon".into(),
            },
            ToolFact::Error {
                text: "server unavailable".into(),
            },
        ])
    );
}

// Covers: `_rho_` hex-escaped MCP names render decoded server and tool
// Owner: interactive presenter
#[test]
fn mcp_escaped_exported_name_renders_decoded_server_and_tool() {
    use rho_tools::tool_card::{ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};

    let dir = tempfile::TempDir::new().unwrap();
    let name = "mcp___rho_6769742d687562___rho_6973737565732f6c697374";
    let view = ToolView {
        kind: ToolKind::from_name(name),
        name: name.into(),
        arguments: serde_json::json!({ "path": "crates" }),
        metadata: Default::default(),
    };
    let card = start_card(&view, dir.path());
    assert_eq!(view.kind, ToolKind::Mcp);
    assert_eq!(
        card,
        ToolCard::new(
            ToolStatus::Running,
            ToolFamily::Default,
            ToolHeader::call("issues/list", Some("crates".into())),
        )
        .with_facts(vec![ToolFact::Meta {
            text: "mcp · git-hub".into(),
        }])
    );
}
