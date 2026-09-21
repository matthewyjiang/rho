use super::*;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn hook(command: Vec<String>) -> HookContractView {
    HookContractView {
        active: true,
        id: "user:check".into(),
        event: "before_tool_use".into(),
        tools: "bash".into(),
        command,
        working_directory: "/work".into(),
        timeout: "2s".into(),
        environment: vec!["PATH".into(), "RHO_IN_HOOK".into()],
    }
}

// Covers: a spaced argv element must stay one token, and a wrapped path must
// continue in the value column. Joining argv with spaces would show a different
// program than the one that runs. The PTY fixture has no spaced argument and is
// too wide to wrap.
// Owner: pure layout
#[test]
fn argv_tokens_stay_intact_and_wrap_under_the_value() {
    let spaced = hook(vec![
        "/bin/sh".into(),
        "has space".into(),
        "--strict".into(),
    ]);
    let spaced_lines: Vec<String> = hook_lines(&spaced, 48).iter().map(line_text).collect();
    let spaced_text = spaced_lines.join("\n");
    assert!(
        spaced_lines.iter().any(|line| line.contains("'has space'")),
        "{spaced_text}"
    );
    assert!(
        !spaced_text.contains("sh has space"),
        "spaced arg was joined as a shell string:\n{spaced_text}"
    );

    let path = "/workspace/hooks/deny-force-push-with-a-very-long-name.sh";
    let wrapped = hook(vec!["/bin/sh".into(), path.into()]);
    let wrapped_lines: Vec<String> = hook_lines(&wrapped, 36).iter().map(line_text).collect();
    let first = wrapped_lines
        .iter()
        .find(|line| line.contains("/bin/sh"))
        .expect("first argv token");
    let value_at = first.find("/bin/sh").expect("argv value column");
    let continuation = wrapped_lines
        .iter()
        .find(|line| line.contains("very-long-name"))
        .expect("wrapped path");
    let leading = continuation.chars().take_while(|ch| *ch == ' ').count();
    assert_eq!(
        leading, value_at,
        "continuation {continuation:?} should start at the value column of {first:?}"
    );
}
