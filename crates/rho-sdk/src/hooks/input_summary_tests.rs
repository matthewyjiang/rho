use pretty_assertions::assert_eq;

use super::*;
use crate::{
    host_input::{HostChoice, HostQuestion, SelectionMode},
    workspace::{
        CapabilityRequest, CapabilitySource, NetworkTarget, PathScope, ProcessEnvironment,
        ProcessExecution, ProcessInvocation, ProcessOutputLimits,
    },
};

fn process(invocation: ProcessInvocation) -> CapabilityRequest {
    CapabilityRequest::process(
        ProcessExecution::new(
            "/repo",
            invocation,
            ProcessEnvironment::InheritAll,
            ProcessOutputLimits::new(/*max_output_bytes*/ 1024, /*timeout*/ None),
        ),
        CapabilitySource::built_in_tool("bash"),
    )
}

fn write(path: &str) -> CapabilityRequest {
    CapabilityRequest::write_path(
        path,
        PathScope::PrimaryWorkspace,
        CapabilitySource::built_in_tool("write"),
    )
}

#[test]
fn an_approval_names_its_operation_and_any_reason_on_one_line() {
    let cases = [
        (write("/repo/note.txt"), "", "write /repo/note.txt"),
        (
            process(ProcessInvocation::shell(
                "/bin/sh",
                vec!["-c".into()],
                "\n  git push --force\nmore lines",
            )),
            "pushes to a remote",
            "run git push --force (pushes to a remote)",
        ),
        (
            process(ProcessInvocation::executable(
                "/usr/bin/cargo",
                vec!["build".into(), "-j".into(), "8".into()],
            )),
            "",
            "run /usr/bin/cargo build -j 8",
        ),
        (
            CapabilityRequest::network(
                NetworkTarget::Url("https://user:secret@example.com/path?token=abc".into()),
                CapabilitySource::built_in_tool("fetch"),
            ),
            "",
            "access example.com",
        ),
        (
            write("/repo/a\nb\u{1b}[2J.txt"),
            "",
            "write /repo/a b [2J.txt",
        ),
    ];
    let summaries: Vec<_> = cases
        .iter()
        .map(|(capability, reason, _)| approval(&ApprovalRequest::new(capability.clone(), *reason)))
        .collect();
    let expected: Vec<_> = cases.iter().map(|(_, _, summary)| *summary).collect();
    assert_eq!(summaries, expected);
}

#[test]
fn an_untitled_questionnaire_is_summarized_by_its_first_prompt() {
    let question = |id: &str, prompt: &str| {
        HostQuestion::new(
            id,
            prompt,
            vec![HostChoice::new("yes", "Yes")],
            SelectionMode::One,
        )
        .unwrap()
    };
    let request = HostInputRequest::questionnaire(
        "",
        vec![
            question("deploy", "Deploy to staging?\nIt is Friday."),
            question("why", "Why?"),
        ],
    )
    .unwrap();
    assert_eq!(super::question(&request), "Deploy to staging?");
}
