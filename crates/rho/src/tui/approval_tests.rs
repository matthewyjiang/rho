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

fn long_process_request() -> CapabilityRequest {
    CapabilityRequest::process(
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
    )
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

// Covers: process approvals lead with the command, keep Deny focused, open at the
// request head on short viewports, and grow detail with the terminal.
// Owner: tui approval layout
#[test]
fn detail_window_starts_at_head_and_grows_with_viewport() {
    let request = long_process_request();
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
            .first()
            .is_some_and(|line| line.contains("wants to run a command")),
        "title should name the process action"
    );
    assert!(
        short
            .get(1)
            .is_some_and(|line| line.contains("printf segment-01")),
        "command must be the first detail row: {short:?}"
    );
    assert!(
        short.iter().any(|line| line.contains("→ Deny")),
        "prompt should focus Deny by default"
    );
    assert!(
        short.iter().all(|line| !line.contains("capability:")),
        "capability class is already in the title and must not repeat as body chrome"
    );
    assert!(
        short.iter().all(|line| !line.contains("ANTHROPIC_API_KEY")),
        "environment scrub lists must stay summarized"
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
    assert!(
        tall.iter().any(|line| line.contains("cwd  /work")),
        "tall viewport should include compact cwd context"
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

// Covers: env scrub lists collapse to a count so secrets never dump into the composer.
// Owner: tui approval layout
#[test]
fn process_environment_summary_hides_variable_names() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], "echo hi"),
            ProcessEnvironment::InheritExcept {
                variable_names: vec![
                    "ANTHROPIC_API_KEY".into(),
                    "OPENAI_API_KEY".into(),
                    "GITHUB_COPILOT_TOKEN".into(),
                ],
            },
            ProcessOutputLimits::new(64_000, None),
        ),
        source(),
    );
    let details = approval_details(&request).join("\n");

    assert!(details.contains("echo hi"));
    assert!(details.contains("env inherit (3 vars stripped)"));
    assert!(details.contains("64 KB out"));
    assert!(!details.contains("ANTHROPIC_API_KEY"));
    assert!(!details.contains("OPENAI_API_KEY"));
    assert!(!details.contains("executable resolution:"));
    assert!(!details.contains("shell invocation"));
}

// Covers: paging can reach a long command suffix and the trailing meta context.
// Owner: tui approval layout
#[test]
fn detail_paging_reaches_command_suffix_and_meta() {
    let request = long_process_request();
    let width = 40;
    let viewport_height = 14;

    let mut saw_suffix = false;
    let mut saw_cwd = false;
    let mut saw_earlier = false;
    for offset in 0..32 {
        let lines = line_text(&approval_lines_for_position(
            &request,
            "",
            ApprovalChoice::Deny,
            offset,
            width,
            viewport_height,
        ));
        saw_suffix |= lines
            .iter()
            .any(|line| line.contains("DANGEROUS_SUFFIX_INSPECTABLE"));
        saw_cwd |= lines.iter().any(|line| line.contains("cwd  /work"));
        saw_earlier |= lines.iter().any(|line| line.contains("↑ earlier"));
        if saw_suffix && saw_cwd && saw_earlier {
            break;
        }
    }

    assert!(
        saw_suffix,
        "PageDown-style offsets must eventually reveal the command suffix"
    );
    assert!(
        saw_cwd,
        "PageDown-style offsets must eventually reveal compact cwd meta"
    );
    assert!(
        saw_earlier,
        "once scrolled past the head, the prompt should offer paging back"
    );

    let end = line_text(&approval_lines_for_position(
        &request,
        "",
        ApprovalChoice::Deny,
        10_000,
        width,
        viewport_height,
    ));
    assert!(
        end.iter().any(|line| line.contains("cwd  /work"))
            || end.iter().any(|line| line.contains("env empty")),
        "oversized offsets should clamp onto the trailing meta window: {end:?}"
    );
    assert!(
        end.iter().any(|line| line.contains("↑ earlier")),
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

    assert_eq!(
        approval_title(&process),
        "ba\\u{2066}sh wants to run a command"
    );
    assert!(details.contains("/work\\u{202e}space"));
    assert!(details.contains("echo safe\\u{200f}danger"));
    assert!(details.contains("sh\\u{2066}"));
    assert!(details.contains("-c\\u{200f}"));
    // Variable names stay out of the summary; only the count is shown.
    assert!(details.contains("env inherit 1 listed var"));
    assert!(!details.contains("PA\\u{202e}TH"));

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
