use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalDecision, CapabilityRequest, CapabilitySource, PathScope, ProcessEnvironment,
    ProcessExecution, ProcessInvocation, ProcessOutputLimits,
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
    assert!(!line_text(&lines).is_empty());
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
