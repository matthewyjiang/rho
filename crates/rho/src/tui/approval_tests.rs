use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalDecision, CapabilityRequest, CapabilitySource, PathScope, ProcessEnvironment,
    ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};

use super::{
    render::{
        approval_detail_page_lines, approval_details, approval_lines_for_position, approval_title,
        format_direct_invocation,
    },
    ApprovalChoice,
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
    assert_eq!(
        ApprovalChoice::AllowOnce.previous(),
        ApprovalChoice::AllowOnce
    );
    assert_eq!(
        ApprovalChoice::Deny.previous(),
        ApprovalChoice::AllowForSession
    );
    assert_eq!(
        ApprovalChoice::AllowOnce.next(),
        ApprovalChoice::AllowForSession
    );
    assert_eq!(ApprovalChoice::AllowForSession.next(), ApprovalChoice::Deny);
    assert_eq!(ApprovalChoice::Deny.next(), ApprovalChoice::Deny);
}

#[test]
fn maps_choices_to_decisions_and_defaults_to_deny() {
    assert_eq!(ApprovalChoice::default(), ApprovalChoice::Deny);
    assert_eq!(
        ApprovalChoice::AllowOnce.decision(),
        ApprovalDecision::AllowOnce
    );
    assert_eq!(
        ApprovalChoice::AllowForSession.decision(),
        ApprovalDecision::AllowForSession
    );
    assert_eq!(
        ApprovalChoice::Deny.decision(),
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
    let lines = approval_lines_for_position(
        &request,
        "a long reason that must wrap",
        ApprovalChoice::AllowForSession,
        0,
        width,
        14,
    );

    assert!(lines.iter().all(|line| line.width() <= width));
    assert!(lines.len() <= 9);
    assert!(!line_text(&lines).is_empty());
}

// Covers: detail window opens at the request head, grows with the terminal, and
// omits empty policy reasons without a UI denylist.
// Owner: tui approval layout
#[test]
fn detail_window_starts_at_head_and_grows_with_viewport() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::shell_from_path(
                "sh",
                vec!["-c".into()],
                "printf segment-01; printf segment-02; printf segment-03; printf segment-04; printf segment-05; echo DANGEROUS_SUFFIX_INSPECTABLE",
            ),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        source(),
    );
    let width = 40;
    let short = line_text(&approval_lines_for_position(
        &request,
        "",
        ApprovalChoice::Deny,
        0,
        width,
        14,
    ));
    let tall = line_text(&approval_lines_for_position(
        &request,
        "",
        ApprovalChoice::Deny,
        0,
        width,
        60,
    ));
    let with_reason = line_text(&approval_lines_for_position(
        &request,
        "custom audit reason",
        ApprovalChoice::Deny,
        0,
        width,
        60,
    ));

    assert!(
        short
            .iter()
            .any(|line| line.contains("capability: process")),
        "prompt should name the capability class"
    );
    assert!(
        short.iter().any(|line| line.contains("> Deny")),
        "prompt should focus Deny by default"
    );
    assert!(
        !short
            .iter()
            .any(|line| line.contains("DANGEROUS_SUFFIX_INSPECTABLE")),
        "short viewport must open on the start of the request, not the suffix"
    );
    assert!(
        tall.iter()
            .any(|line| line.contains("DANGEROUS_SUFFIX_INSPECTABLE")),
        "tall viewport should expose more of the request without paging"
    );
    assert!(approval_detail_page_lines(14) >= 3);
    assert!(approval_detail_page_lines(60) > approval_detail_page_lines(14));
    assert!(
        !short.iter().any(|line| line.contains("reason:")),
        "empty policy reasons must not render a reason row"
    );
    assert!(
        with_reason
            .iter()
            .any(|line| line.contains("reason: custom audit reason")),
        "non-empty reasons should still render"
    );
}

// Covers: stale offsets after a larger page size must clamp instead of sticking.
// Owner: tui approval layout
#[test]
fn detail_offset_clamps_when_page_grows() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::shell_from_path(
                "sh",
                vec!["-c".into()],
                "printf segment-01; printf segment-02; printf segment-03; printf segment-04; printf segment-05; echo DANGEROUS_SUFFIX_INSPECTABLE",
            ),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        source(),
    );
    let width = 40;
    let lines = line_text(&approval_lines_for_position(
        &request,
        "",
        ApprovalChoice::Deny,
        10_000,
        width,
        14,
    ));

    assert!(
        lines
            .iter()
            .any(|line| line.contains("DANGEROUS_SUFFIX_INSPECTABLE")),
        "oversized offsets should clamp to the final visible window"
    );
    assert!(
        lines.iter().any(|line| line.contains("↑ earlier")),
        "clamped end window on a short viewport should still offer paging back"
    );
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
