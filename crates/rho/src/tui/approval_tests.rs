use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_protocol::{ApprovalAsk, ApprovalSource, Operation, PathScope};
use rho_sdk::ApprovalDecision;

use super::{
    render::{approval_detail_page_lines, approval_frame_for_position, format_direct_invocation},
    ApprovalChoice, ApprovalPrompt,
};

fn bash() -> ApprovalSource {
    ApprovalSource::BuiltInTool {
        name: "bash".into(),
    }
}

fn prompt(source: ApprovalSource, operation: Operation, reason: &str) -> ApprovalPrompt {
    ApprovalPrompt::of_daemon_ask(&ApprovalAsk {
        source,
        operation,
        reason: reason.into(),
    })
}

/// Detail rows, primary first, as flat strings.
fn details(prompt: &ApprovalPrompt) -> Vec<String> {
    let mut flat = prompt.details.primary.clone();
    flat.extend(prompt.details.meta.iter().cloned());
    flat
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

fn write_path(path: &str) -> Operation {
    Operation::WritePath {
        path: path.into(),
        scope: PathScope::Workspace,
    }
}

fn shell(cwd: &str, program: &str, flag: &str, command: &str) -> Operation {
    Operation::RunProcess {
        program: program.into(),
        args: vec![flag.into(), command.into()],
        shell_command: Some(command.into()),
        cwd: cwd.into(),
    }
}

fn long_process() -> Operation {
    shell(
        "/work",
        "sh",
        "-c",
        "printf segment-01; printf segment-02; printf segment-03; printf segment-04; printf segment-05; echo DANGEROUS_SUFFIX_INSPECTABLE",
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
    let width = 14;
    let lines = approval_frame_for_position(
        &prompt(
            bash(),
            write_path("src/a-very-long-directory/main.rs"),
            "a long reason that must wrap",
        ),
        ApprovalChoice::AllowForSession,
        0,
        width,
        14,
    )
    .lines;

    assert!(lines.iter().all(|line| line.width() <= width));
    assert!(lines.len() <= 9);
    assert!(!line_text(&lines).is_empty());
}

// Covers: process approvals lead with the command, open at the request head on
// short viewports, and grow detail with the terminal.
// Owner: tui approval layout
#[test]
fn detail_window_starts_at_head_and_grows_with_viewport() {
    let width = 40;
    let frame = |reason: &str, viewport_height| {
        line_text(
            &approval_frame_for_position(
                &prompt(bash(), long_process(), reason),
                ApprovalChoice::Deny,
                0,
                width,
                viewport_height,
            )
            .lines,
        )
    };
    let short = frame("", 14);
    let tall = frame("", 60);
    let with_reason = frame("custom audit reason", 60);

    assert!(
        short
            .get(1)
            .is_some_and(|line| line.contains("printf segment-01")),
        "command must be the first detail row: {short:?}"
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
        !short.iter().any(|line| line.contains("reason")),
        "empty policy reasons must not render a reason row"
    );
    assert!(
        with_reason
            .iter()
            .any(|line| line.contains("custom audit reason")),
        "non-empty reasons should still render"
    );
}

// Covers: every approval leads with its bare primary target (JSON argv for
// direct processes, then path, URL, or skill name) so the user sees what is
// being approved before any meta.
// Owner: tui approval layout
#[test]
fn approvals_lead_with_the_primary_target() {
    let cases = [
        (
            Operation::RunProcess {
                program: "/usr/bin/git".into(),
                args: vec!["status".into(), "--short".into()],
                shell_command: None,
                cwd: "/work".into(),
            },
            r#"["/usr/bin/git", "status", "--short"]"#,
        ),
        (write_path("src/main.rs"), "src/main.rs"),
        (
            Operation::Network {
                url: Some("https://example.test".into()),
            },
            "https://example.test",
        ),
        (
            Operation::LoadSkill {
                name: "demo-skill".into(),
                path: Some(PathBuf::from("/skills/demo/SKILL.md")),
            },
            "demo-skill",
        ),
    ];
    for (operation, primary) in cases {
        assert_eq!(details(&prompt(bash(), operation, ""))[0], primary);
    }
}

// Covers: host-provided tools are labeled in the title without burying the
// command/path primary body; built-ins stay unmarked.
// Owner: tui approval layout
#[test]
fn title_marks_host_provided_source_without_changing_primary() {
    let host_tool = |name: &str| ApprovalSource::HostTool { name: name.into() };
    let built_in = prompt(
        ApprovalSource::BuiltInTool {
            name: "workspace_patch".into(),
        },
        write_path("src/main.rs"),
        "",
    );
    let host = prompt(host_tool("workspace_patch"), write_path("src/main.rs"), "");
    let host_hostile = prompt(
        host_tool("evil\u{202e}tool"),
        shell("/work", "sh", "-c", "echo ok"),
        "",
    );

    assert_ne!(host.title, built_in.title);
    assert_eq!(
        host_hostile.title,
        "host evil\\u{202e}tool wants to run a command"
    );

    // Primary body stays the path/command; provenance lives only in the title.
    assert_eq!(details(&host), details(&built_in));
    assert_eq!(details(&host_hostile)[0], "echo ok");
}

// Covers: paging can reach a long command's suffix and its cwd meta, and an
// oversized offset clamps onto the last window.
// Owner: tui approval layout
#[test]
fn detail_paging_reaches_command_suffix_and_meta() {
    let width = 40;
    let viewport_height = 14;
    let frame = |offset| {
        line_text(
            &approval_frame_for_position(
                &prompt(bash(), long_process(), ""),
                ApprovalChoice::Deny,
                offset,
                width,
                viewport_height,
            )
            .lines,
        )
    };

    let mut saw_suffix = false;
    let mut saw_cwd = false;
    let mut saw_earlier = false;
    for offset in 0..32 {
        let lines = frame(offset);
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

    let end = frame(10_000);
    assert!(
        end.iter()
            .any(|line| line.contains("DANGEROUS_SUFFIX_INSPECTABLE")),
        "oversized offsets should clamp onto the last detail window: {end:?}"
    );
    assert!(
        end.iter().any(|line| line.contains("↑ earlier")),
        "clamped end window on a short viewport should still offer paging back"
    );
}

#[test]
fn escapes_unicode_format_controls_in_all_security_sensitive_fields() {
    let process = prompt(
        ApprovalSource::BuiltInTool {
            name: "ba\u{2066}sh".into(),
        },
        shell(
            "/work\u{202e}space",
            "sh\u{2066}",
            "-c\u{200f}",
            "echo safe\u{200f}danger",
        ),
        "",
    );
    let shown = details(&process).join("\n");

    assert_eq!(process.title, "ba\\u{2066}sh wants to run a command");
    assert!(shown.contains("/work\\u{202e}space"));
    assert!(shown.contains("echo safe\\u{200f}danger"));
    assert!(shown.contains("sh\\u{2066}"));
    assert!(shown.contains("-c\\u{200f}"));

    let path = prompt(bash(), write_path("safe\u{202e}txt"), "");
    assert!(details(&path)[0].contains("safe\\u{202e}txt"));

    assert_eq!(
        format_direct_invocation(
            std::path::Path::new("tool\u{2066}"),
            &["arg\u{200f}".into(), "tail\u{202e}".into()]
        ),
        r#"["tool\u{2066}", "arg\u{200f}", "tail\u{202e}"]"#
    );
}
