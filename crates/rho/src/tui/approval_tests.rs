use pretty_assertions::assert_eq;
use rho_sdk::{
    ApprovalDecision, CapabilityRequest, CapabilitySource, NetworkTarget, PathScope,
    ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits,
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
        tall.iter().any(|line| line.contains("cwd /work")),
        "tall viewport should include compact cwd context on the single meta line"
    );
    assert!(approval_detail_page_lines(14) >= 3);
    assert!(approval_detail_page_lines(60) > approval_detail_page_lines(14));
    assert!(
        !short.iter().any(|line| line.contains("reason")),
        "empty policy reasons must not render a reason row"
    );
    assert!(
        with_reason
            .iter()
            .any(|line| line.contains("reason custom audit reason")),
        "non-empty reasons should still render"
    );
}

// Covers: env scrub lists collapse to a count; allowlists keep sanitized names;
// process meta is one packed line with JSON via-args and raw byte limits.
// Owner: tui approval layout
#[test]
fn process_meta_is_one_line_and_summarizes_env_modes() {
    let scrubbed = CapabilityRequest::process(
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
    let scrubbed_details = approval_details(&scrubbed);
    assert_eq!(scrubbed_details.len(), 2, "{scrubbed_details:?}");
    assert_eq!(scrubbed_details[0], "echo hi");
    let scrubbed_meta = &scrubbed_details[1];
    assert!(scrubbed_meta.contains("cwd /work"), "{scrubbed_meta}");
    assert!(
        scrubbed_meta.contains(r#"via ["bash", "-lc"] (PATH)"#),
        "{scrubbed_meta}"
    );
    assert!(
        scrubbed_meta.contains("env inherit (3 stripped)"),
        "{scrubbed_meta}"
    );
    assert!(scrubbed_meta.contains("64000 B out"), "{scrubbed_meta}");
    assert!(scrubbed_meta.contains("no timeout"), "{scrubbed_meta}");
    assert!(!scrubbed_meta.contains("ANTHROPIC_API_KEY"));
    assert!(!scrubbed_meta.contains("executable resolution:"));
    assert!(!scrubbed_meta.contains("shell invocation"));

    let allowlisted = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], "echo hi"),
            ProcessEnvironment::InheritListed {
                variable_names: vec!["PATH".into(), "HOME".into()],
            },
            ProcessOutputLimits::new(1024, None),
        ),
        source(),
    );
    let allowlisted_meta = &approval_details(&allowlisted)[1];
    assert!(
        allowlisted_meta.contains(r#"env ["PATH", "HOME"]"#),
        "{allowlisted_meta}"
    );
}

// Covers: direct (non-shell) process approvals lead with JSON argv and pack
// lookup/env/limits onto one meta line without a redundant via-payload.
// Owner: tui approval layout
#[test]
fn direct_process_approval_leads_with_json_argv() {
    let request = CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::executable("/usr/bin/git", vec!["status".into(), "--short".into()]),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(2048, Some(std::time::Duration::from_secs(30))),
        ),
        source(),
    );
    let details = approval_details(&request);
    assert_eq!(details.len(), 2, "{details:?}");
    assert_eq!(details[0], r#"["/usr/bin/git", "status", "--short"]"#);
    assert!(
        details[1].contains("cwd /work")
            && details[1].contains("exact path")
            && details[1].contains("env empty")
            && details[1].contains("2048 B out")
            && details[1].contains("timeout 30s")
            && !details[1].contains("via "),
        "direct meta: {}",
        details[1]
    );
}

// Covers: path/network/skill approvals lead with the bare primary target.
// Owner: tui approval layout
#[test]
fn non_process_approvals_lead_with_bare_primary() {
    let path = CapabilityRequest::write_path("src/main.rs", PathScope::PrimaryWorkspace, source());
    let path_details = approval_details(&path);
    assert_eq!(path_details[0], "src/main.rs");
    assert_eq!(path_details[1], "scope primary workspace");

    let network =
        CapabilityRequest::network(NetworkTarget::Url("https://example.test".into()), source());
    let network_details = approval_details(&network);
    assert_eq!(network_details, vec!["https://example.test".to_string()]);

    let skill = CapabilityRequest::skill(
        "demo-skill",
        Some(std::path::PathBuf::from("/skills/demo/SKILL.md")),
        source(),
    );
    let skill_details = approval_details(&skill);
    assert_eq!(skill_details[0], "demo-skill");
    assert_eq!(skill_details[1], "path /skills/demo/SKILL.md");
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
        saw_cwd |= lines.iter().any(|line| line.contains("cwd /work"));
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
        end.iter().any(|line| line.contains("cwd /work")),
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
    // Allowlist names stay visible after sanitize so reviewers can audit them.
    assert!(details.contains("PA\\u{202e}TH"));

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
