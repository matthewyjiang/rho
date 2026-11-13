use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{Location, ToolKind};
use rho_sdk::tool::{OperationKind, ToolFileChange, ToolMetadata, ToolOutput};
use serde_json::json;

use super::{finished_locations, kind, started_locations};

fn location(path: &str, line: Option<u32>) -> Location {
    Location {
        path: PathBuf::from(path),
        line,
    }
}

// Covers: kinds beyond the operation map: a patch that only deletes or only
// moves files, the checklist as planning, and plan exit as a mode switch.
// A patch that does anything else, or cannot parse, stays an edit.
// Owner: session event tool presentation
#[test]
fn started_calls_name_their_kind() {
    let write = || ToolMetadata::new().operation(OperationKind::Write);
    let patch = |body: &str| json!({"input": format!("*** Begin Patch\n{body}*** End Patch")});
    let cases = [
        ("write", None, write(), ToolKind::Edit),
        (
            "apply_patch",
            Some(patch("*** Delete File: a.txt\n*** Delete File: b.txt\n")),
            write(),
            ToolKind::Delete,
        ),
        (
            "apply_patch",
            Some(patch(
                "*** Update File: a.txt\n*** Move to: b.txt\n@@\n-a\n+b\n",
            )),
            write(),
            ToolKind::Move,
        ),
        (
            "apply_patch",
            Some(patch(
                "*** Delete File: a.txt\n*** Update File: b.txt\n@@\n-a\n+b\n",
            )),
            write(),
            ToolKind::Edit,
        ),
        (
            "apply_patch",
            Some(json!({"input": "not a patch"})),
            write(),
            ToolKind::Edit,
        ),
        (
            "edit",
            Some(json!({"input": "*** Delete File: a.txt"})),
            write(),
            ToolKind::Edit,
        ),
        (
            "todo",
            Some(json!({"todos": []})),
            ToolMetadata::new(),
            ToolKind::Think,
        ),
        (
            "exit_plan_mode",
            Some(json!({"plan": "p"})),
            ToolMetadata::new(),
            ToolKind::SwitchMode,
        ),
        (
            "agent",
            None,
            ToolMetadata::new().operation(OperationKind::Other("agent".into())),
            ToolKind::Other,
        ),
    ];
    for (name, arguments, metadata, expected) in cases {
        assert_eq!(
            kind(name, arguments.as_ref(), &metadata),
            expected,
            "{name} {arguments:?}"
        );
    }
}

// Covers: started locations are absolute, and a read names the 0-based row
// its 1-based `offset` starts at; another tool's `offset` is no line.
// Owner: session event tool presentation
#[test]
fn started_locations_are_absolute_and_a_read_names_its_line() {
    let cwd = Path::new("/work");
    let cases = [
        (
            "read_file",
            json!({"path": "src/a.rs", "offset": 10}),
            "src/a.rs",
            location("/work/src/a.rs", Some(9)),
        ),
        (
            "read_file",
            json!({"path": "/abs/a.rs"}),
            "/abs/a.rs",
            location("/abs/a.rs", None),
        ),
        (
            "process",
            json!({"path": "log.txt", "offset": 10}),
            "log.txt",
            location("/work/log.txt", None),
        ),
    ];
    for (name, arguments, path, expected) in cases {
        let metadata = ToolMetadata::new().affected_path(path);
        assert_eq!(
            started_locations(name, Some(&arguments), &metadata, cwd),
            vec![expected],
            "{name}"
        );
    }
}

// Covers: a finished edit points at each changed file's first changed row
// (0-based): after a change, an append, or a cut from the end. New, emptied,
// and unchanged files have no line. Output naming no files keeps the started
// locations.
// Owner: session event tool presentation
#[test]
fn finished_edits_point_at_the_first_changed_line() {
    let cases = [
        (
            ToolFileChange::modified("a.txt", "a\nb\nc\n", "a\nB\nc\n"),
            Some(1),
        ),
        (
            ToolFileChange::modified("a.txt", "a\r\nb", "a\r\nb\r\nc"),
            Some(2),
        ),
        (
            ToolFileChange::modified("a.txt", "a\nb\nc", "a\nb"),
            Some(1),
        ),
        (ToolFileChange::modified("a.txt", "a\n", "a\n"), None),
        (ToolFileChange::created("a.txt", "new\n"), None),
        (ToolFileChange::deleted("a.txt", "old\n"), None),
    ];
    for (change, line) in cases {
        let output = ToolOutput::text("ok").metadata(ToolMetadata::new().file_change(change));
        assert_eq!(
            finished_locations(&output, Path::new("/work")),
            Some(vec![location("/work/a.txt", line)]),
            "{line:?}"
        );
    }
    let plain = ToolOutput::text("ok").metadata(
        ToolMetadata::new()
            .operation(OperationKind::Search)
            .affected_path("src"),
    );
    assert_eq!(finished_locations(&plain, Path::new("/work")), None);
}
