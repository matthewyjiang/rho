use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalDecision, CapabilityRequest, CapabilitySource, NetworkTarget, PathScope,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};

use super::{
    approval_decision, next_choice, previous_choice,
    render::{
        approval_details, approval_lines_for_position, approval_title, format_direct_invocation,
    },
};

fn source() -> CapabilitySource {
    CapabilitySource::built_in_tool("bash")
}

fn line_text(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[test]
fn movement_stops_at_choice_boundaries() {
    assert_eq!(previous_choice(0), 0);
    assert_eq!(previous_choice(2), 1);
    assert_eq!(next_choice(0), 1);
    assert_eq!(next_choice(1), 2);
    assert_eq!(next_choice(2), 2);
}

#[test]
fn maps_choice_indices_to_decisions() {
    assert_eq!(approval_decision(0), ApprovalDecision::AllowOnce);
    assert_eq!(approval_decision(1), ApprovalDecision::AllowForSession);
    assert_eq!(
        approval_decision(2),
        ApprovalDecision::Deny {
            reason: "denied by user".into()
        }
    );
}

#[test]
fn derives_titles_for_capability_kinds_and_sources() {
    let requests = [
        (
            CapabilityRequest::write_path("file", PathScope::PrimaryWorkspace, source()),
            "bash wants to write",
        ),
        (
            CapabilityRequest::read_path("file", PathScope::PrimaryWorkspace, source()),
            "bash wants to read",
        ),
        (
            CapabilityRequest::network(NetworkTarget::ToolManaged, source()),
            "bash wants to access the network",
        ),
        (
            CapabilityRequest::skill("review", None, source()),
            "bash wants to load a skill",
        ),
        (
            CapabilityRequest::instruction_discovery(
                ".",
                PathScope::PrimaryWorkspace,
                CapabilitySource::PromptConstruction,
            ),
            "rho wants to discover instructions",
        ),
    ];

    for (request, expected) in requests {
        assert_eq!(approval_title(&request), expected);
    }

    let process = CapabilityRequest::process(
        ProcessExecution::new(
            ".",
            ProcessInvocation::executable_from_path("cargo", vec!["test".into()]),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::host_tool("shell"),
    );
    assert_eq!(approval_title(&process), "shell wants to execute");
}

#[test]
fn renders_complete_sanitized_operation_details() {
    let write = CapabilityRequest::write_path(
        "src/main.rs\nspoofed",
        PathScope::PrimaryWorkspace,
        source(),
    );
    assert_eq!(
        approval_details(&write),
        vec!["path: src/main.rs\\nspoofed", "scope: primary workspace",]
    );

    let network = CapabilityRequest::network(
        NetworkTarget::Url("https://example.com/a/very/long/path".into()),
        source(),
    );
    assert_eq!(
        approval_details(&network),
        vec!["target: https://example.com/a/very/long/path"]
    );

    let process = CapabilityRequest::process(
        ProcessExecution::new(
            "/workspace/project",
            ProcessInvocation::shell_from_path(
                "sh",
                vec!["-c".into()],
                "printf 'safe'\nrm -rf -- /dangerous-suffix",
            ),
            ProcessEnvironment::InheritListed {
                variable_names: vec!["PATH".into(), "LANG".into()],
            },
            ProcessOutputLimits::new(1024, None),
        ),
        source(),
    );
    assert_eq!(
        approval_details(&process),
        vec![
            "working directory: /workspace/project",
            "executable resolution: PATH search",
            "environment: inherit listed variable names [\"PATH\", \"LANG\"]",
            "output limit: 1024 bytes; timeout: none",
            "shell invocation (JSON-style args): [\"sh\", \"-c\"]",
            "command: printf 'safe'\\nrm -rf -- /dangerous-suffix",
        ]
    );
}

#[test]
fn direct_invocation_formatter_preserves_argument_boundaries() {
    let arguments = vec![
        "with spaces".into(),
        "a\"quote".into(),
        String::new(),
        "日本語".into(),
    ];

    assert_eq!(
        format_direct_invocation(std::path::Path::new("tool name"), &arguments),
        r#"["tool name", "with spaces", "a\"quote", "", "日本語"]"#
    );
}

#[test]
fn renders_default_choice_and_footer() {
    let request =
        CapabilityRequest::write_path("src/main.rs", PathScope::PrimaryWorkspace, source());
    let lines = line_text(&approval_lines_for_position(
        &request,
        "needs editing",
        0,
        0,
        80,
    ));

    assert_eq!(
        lines,
        vec![
            "bash wants to write",
            "path: src/main.rs",
            "scope: primary workspace",
            "reason: needs editing",
            "> Allow once",
            "  Allow for session",
            "  Deny",
            "pgup/pgdn details 2-4/4 · ↑ earlier",
            "enter confirm · arrows choose · esc deny & cancel",
        ]
    );
}

#[test]
fn bounded_detail_pages_keep_controls_visible_and_default_to_suffix() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/workspace/with-a-long-directory",
            ProcessInvocation::shell_from_path(
                "sh",
                vec!["-c".into()],
                "printf safe && remove -- /dangerous/final/suffix",
            ),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        source(),
    );
    let width = 36;
    let end_page = line_text(&approval_lines_for_position(&request, "", 0, 0, width));
    let earlier_page = line_text(&approval_lines_for_position(&request, "", 0, 3, width));

    for page in [&end_page, &earlier_page] {
        assert!(page.len() <= 9);
        assert!(page.iter().any(|line| line.contains("Allow once")));
        assert!(page.iter().any(|line| line.contains("Allow for session")));
        assert!(page.iter().any(|line| line.contains("Deny")));
        assert!(page.iter().any(|line| line.contains("pgup/pgdn details ")));
    }
    assert!(end_page.join("").contains("/dangerous/final/suffix"));
    assert!(
        end_page
            .iter()
            .any(|line| line.contains("earlier") || line.contains('↑')),
        "{end_page:?}"
    );
    assert!(earlier_page.iter().any(|line| line.contains("↓ later")));
}

#[test]
fn paging_makes_every_wrapped_detail_inspectable() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/workspace/with-a-long-directory",
            ProcessInvocation::shell_from_path(
                "sh",
                vec!["-c".into()],
                "printf safe && remove -- /dangerous/final/suffix",
            ),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        source(),
    );
    let rendered = (0..100)
        .rev()
        .flat_map(|page| {
            line_text(&approval_lines_for_position(&request, "", 0, page, 12))
                .into_iter()
                .skip(1)
                .take(3)
        })
        .collect::<Vec<_>>()
        .join("");

    assert!(
        rendered.contains("/workspace/with-a-long-directory"),
        "{rendered}"
    );
    assert!(rendered.contains("remove -- /dangerous/final/suffix"));
}

#[test]
fn every_rendered_line_respects_narrow_width() {
    let request = CapabilityRequest::write_path(
        "src/a-very-long-directory/main.rs",
        PathScope::PrimaryWorkspace,
        source(),
    );
    let width = 14;
    let lines = approval_lines_for_position(&request, "a long reason that must wrap", 1, 0, width);

    assert!(lines.iter().all(|line| line.width() <= width));
    assert!(lines.len() <= 9);
    let rendered = line_text(&lines).join("");
    assert!(rendered.contains("Allow once"));
    assert!(rendered.contains("Allow for"));
    assert!(rendered.contains("Deny"));
}

#[test]
fn escapes_unicode_format_controls_in_all_security_sensitive_fields() {
    let process = CapabilityRequest::process(
        ProcessExecution::new(
            "/work\u{202e}space",
            ProcessInvocation::shell_from_path(
                "sh\u{2066}",
                vec!["-c\u{200f}".into()],
                "echo safe\u{200f}danger",
            ),
            ProcessEnvironment::InheritListed {
                variable_names: vec!["PA\u{202e}TH".into()],
            },
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool("ba\u{2066}sh"),
    );
    let details = approval_details(&process).join("\n");

    assert_eq!(approval_title(&process), "ba\\u{2066}sh wants to execute");
    assert!(details.contains("/work\\u{202e}space"));
    assert!(details.contains("echo safe\\u{200f}danger"));
    assert!(details.contains(r#"["sh\u{2066}", "-c\u{200f}"]"#));
    assert!(details.contains(r#"["PA\u{202e}TH"]"#));

    let path =
        CapabilityRequest::write_path("safe\u{202e}txt", PathScope::PrimaryWorkspace, source());
    assert!(approval_details(&path)[0].contains("safe\\u{202e}txt"));

    assert_eq!(
        format_direct_invocation(
            std::path::Path::new("tool\u{2066}"),
            &["arg\u{200f}".into(), "tail\u{202e}".into()]
        ),
        r#"["tool\u{2066}", "arg\u{200f}", "tail\u{202e}"]"#
    );
}
