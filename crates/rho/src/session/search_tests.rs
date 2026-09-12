use std::{fs, io::Write, path::Path};

use pretty_assertions::assert_eq;
use rho_providers::model::Message;
use serde_json::{json, Value};
use tempfile::TempDir;

use super::*;
use crate::session::{search_evidence, Session};

fn run(root: &Path, cwd: &Path, current: &str, args: Value) -> Value {
    serde_json::from_str(
        &execute(
            root,
            cwd,
            current,
            serde_json::from_value(args).unwrap(),
            rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
            &CancellationToken::new(),
        )
        .unwrap(),
    )
    .unwrap()
}

fn ids(value: &Value) -> Vec<&str> {
    value["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect()
}

// Covers: grouping/pagination and byte-budget reduction must not hide omitted
// results or change ordering. Owner: session search response contract.
#[test]
fn paginated_groups_keep_total_and_budget_omissions_visible() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    for _ in 0..3 {
        let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
        session
            .append_message(&Message::user_text("pageword evidence"))
            .unwrap();
    }
    let all = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"pageword"}),
    );
    let expected = ids(&all);
    for offset in 0..=expected.len() {
        let page = run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"pageword","limit":1,"offset":offset}),
        );
        assert_eq!(page["total_sessions"], expected.len());
        assert_eq!(
            ids(&page),
            expected
                .iter()
                .skip(offset)
                .take(1)
                .copied()
                .collect::<Vec<_>>()
        );
    }
    // Derive the test budget from the measured response, not a guessed cap.
    let budget = serde_json::to_string(&all).unwrap().len() * 3 / 4;
    let bounded = execute(
        root.path(),
        cwd.path(),
        "current",
        serde_json::from_value(json!({"action":"search","query":"pageword"})).unwrap(),
        budget,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(bounded.len() <= budget);
    let bounded: Value = serde_json::from_str(&bounded).unwrap();
    let returned = ids(&bounded).len();
    assert!(returned < expected.len());
    assert_eq!(
        (
            bounded["total_sessions"].clone(),
            bounded["next_offset"].clone()
        ),
        (json!(expected.len()), json!(returned))
    );
}

// Covers: a prior transcript must become searchable after append, replacement,
// partial-tail recovery and deletion without scanning unchanged content. Anchors
// survive appends but reject replacement evidence at the same position.
// Owner: session storage/search integration, not rendering.
#[test]
fn indexed_evidence_tracks_source_changes_without_mutating_transcripts() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message_with_display(
            &Message::user_text("providersecret"),
            &Message::user_text("cancellation evidence"),
        )
        .unwrap();
    let original = fs::read(session.path()).unwrap();
    let first = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"cancellation"}),
    );
    assert_eq!(ids(&first), vec![session.id()]);
    assert_eq!(first["index"]["files_updated"], 1);
    let original_read = json!({"action":"read","session":first["sessions"][0]["session"],
        "anchor":first["sessions"][0]["excerpts"][0]["anchor"]});
    let second = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"cancellation"}),
    );
    assert_eq!(first["sessions"], second["sessions"]);
    assert_eq!(second["index"]["bytes_read"], 0);
    assert_eq!(second["index"]["files_checked"], 0);
    assert_eq!(fs::read(session.path()).unwrap(), original);
    assert_eq!(
        ids(&run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"providersecret"})
        )),
        Vec::<&str>::new()
    );

    session
        .append_message(&Message::assistant_text("E0308 repair"))
        .unwrap();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(execute(
        root.path(),
        cwd.path(),
        "current",
        serde_json::from_value(json!({"action":"search","query":"E0308"})).unwrap(),
        rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
        &cancelled
    )
    .is_err());
    assert_eq!(
        ids(&run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"E0308"})
        )),
        vec![session.id()]
    );
    // Appending must keep existing anchors valid, but rewriting the message at
    // the same byte offset must not let an old anchor return different evidence.
    assert_eq!(
        run(root.path(), cwd.path(), "current", original_read.clone())["text"],
        "cancellation evidence"
    );
    let replacement = String::from_utf8(original.clone())
        .unwrap()
        .replace("cancellation evidence", "replacement evidence");
    fs::write(session.path(), replacement).unwrap();
    assert_eq!(
        ids(&run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"E0308","refresh":true})
        )),
        Vec::<&str>::new()
    );
    assert!(execute(
        root.path(),
        cwd.path(),
        "current",
        serde_json::from_value(original_read).unwrap(),
        rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
        &CancellationToken::new(),
    )
    .is_err());
    let replaced = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"replacement"}),
    );
    assert_eq!(ids(&replaced), vec![session.id()]);
    let replacement_read = json!({"action":"read","session":replaced["sessions"][0]["session"],
        "anchor":replaced["sessions"][0]["excerpts"][0]["anchor"]});
    assert_eq!(
        run(root.path(), cwd.path(), "current", replacement_read)["text"],
        "replacement evidence"
    );

    let tail = serde_json::to_string(
        &json!({"type":"message","message":{"User":[{"Text":"tailneedle"}]}}),
    )
    .unwrap();
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(session.path())
        .unwrap();
    file.write_all(tail.as_bytes()).unwrap();
    let partial = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"tailneedle","refresh":true}),
    );
    assert_eq!(
        (ids(&partial), partial["index"]["omitted_records"].clone()),
        (vec![], json!(1))
    );
    file.write_all(b"\n").unwrap();
    assert_eq!(
        ids(&run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"tailneedle","refresh":true})
        )),
        vec![session.id()]
    );
    fs::remove_file(session.path()).unwrap();
    assert_eq!(
        ids(&run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"tailneedle","refresh":true})
        )),
        Vec::<&str>::new()
    );
}

// Covers: an existing derived cache must rebuild positional anchors once rather
// than preserve unsafe handles through the unchanged-file shortcut.
// Owner: session search cache migration.
#[test]
fn positional_anchor_cache_rebuilds_once() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("migrationneedle"))
        .unwrap();
    let args = json!({"action":"search","query":"migrationneedle"});
    let original = run(root.path(), cwd.path(), "current", args.clone());
    {
        let connection = Connection::open(root.path().join("search.sqlite3")).unwrap();
        // The table layout is unchanged; v1 differs in its cached anchor format.
        connection
            .execute_batch("update evidence set anchor='10:0'; pragma user_version=1;")
            .unwrap();
    }
    let migrated = run(root.path(), cwd.path(), "current", args.clone());
    assert_eq!(migrated["sessions"], original["sessions"]);
    assert_eq!(migrated["index"]["files_updated"], 1);
    assert_eq!(migrated["index"]["reconciled"], true);
    let warm = run(root.path(), cwd.path(), "current", args);
    assert_eq!(warm["index"]["files_checked"], 0);
    assert_eq!(warm["index"]["bytes_read"], 0);
}

// Covers: explicit refresh must repair scope after Git metadata changes without
// rereading unchanged transcripts. Owner: session search integration.
#[test]
fn refresh_resolves_scope_again_for_unchanged_transcripts() {
    let root = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let cwd = project.path().join("nested");
    fs::create_dir(&cwd).unwrap();
    let session = Session::create_in_root(root.path(), &cwd).unwrap();
    session
        .append_message(&Message::user_text("scopechange"))
        .unwrap();
    let original = run(
        root.path(),
        &cwd,
        "current",
        json!({"action":"search","query":"scopechange"}),
    );
    assert_eq!(ids(&original), vec![session.id()]);
    let git = project.path().join(".git");
    let admin = TempDir::new().unwrap();
    let metadata = admin.path().join("metadata");
    let moved_metadata = admin.path().join("moved");
    for change in ["init", "remove", "link", "move"] {
        match change {
            "init" => fs::create_dir(&git).unwrap(),
            "remove" => fs::remove_dir(&git).unwrap(),
            "link" => {
                fs::create_dir(&metadata).unwrap();
                fs::write(&git, format!("gitdir: {}", metadata.display())).unwrap();
            }
            "move" => {
                fs::rename(&metadata, &moved_metadata).unwrap();
                fs::write(&git, format!("gitdir: {}", moved_metadata.display())).unwrap();
            }
            _ => unreachable!(),
        }
        for scope in ["repo", "worktree"] {
            let result = run(
                root.path(),
                &cwd,
                "current",
                json!({"action":"search","query":"scopechange","scope":scope,"refresh":true}),
            );
            assert_eq!(ids(&result), vec![session.id()]);
            assert_eq!(result["index"]["bytes_read"], 0);
            let read = run(
                root.path(),
                &cwd,
                "current",
                json!({"action":"read","session":original["sessions"][0]["session"],"anchor":original["sessions"][0]["excerpts"][0]["anchor"],"scope":scope}),
            );
            assert_eq!(read["text"], "scopechange");
        }
    }
}

// Covers: default repo isolation, explicit expansion, same-worktree ordering and
// current-session exclusion must also apply to focused reads.
#[test]
fn scope_and_current_exclusion_apply_to_search_and_read() {
    let root = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    fs::create_dir(project.path().join(".git")).unwrap();
    let linked = TempDir::new().unwrap();
    let admin = project.path().join(".git/worktrees/linked");
    fs::create_dir_all(&admin).unwrap();
    fs::write(admin.join("commondir"), "../..").unwrap();
    fs::write(
        linked.path().join(".git"),
        format!("gitdir: {}", admin.display()),
    )
    .unwrap();
    let sessions = [project.path(), linked.path(), other.path()].map(|cwd| {
        let session = Session::create_in_root(root.path(), cwd).unwrap();
        session
            .append_message(&Message::user_text("scopeword"))
            .unwrap();
        session
    });
    let current = Session::create_in_root(root.path(), project.path()).unwrap();
    current
        .append_message(&Message::user_text("scopeword"))
        .unwrap();
    for (scope, expected) in [
        ("repo", vec![sessions[0].id(), sessions[1].id()]),
        ("worktree", vec![sessions[0].id()]),
    ] {
        let result = run(
            root.path(),
            project.path(),
            current.id(),
            json!({"action":"search","query":"scopeword","scope":scope}),
        );
        assert_eq!(ids(&result), expected);
    }
    let all = run(
        root.path(),
        project.path(),
        current.id(),
        json!({"action":"search","query":"scopeword","scope":"all"}),
    );
    assert_eq!(all["sessions"].as_array().unwrap().len(), 3);
    let foreign = all["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == sessions[2].id())
        .unwrap();
    let args = json!({"action":"read","session":foreign["session"],"anchor":foreign["excerpts"][0]["anchor"]});
    assert!(execute(
        root.path(),
        project.path(),
        current.id(),
        serde_json::from_value(args.clone()).unwrap(),
        rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
        &CancellationToken::new()
    )
    .is_err());
    let mut args = args;
    args["scope"] = json!("all");
    assert_eq!(
        run(root.path(), project.path(), current.id(), args)["text"],
        "scopeword"
    );
    let local = &all["sessions"][0];
    let args = json!({"action":"read","session":local["session"],"anchor":local["excerpts"][0]["anchor"],"scope":"all"});
    assert!(execute(
        root.path(),
        project.path(),
        sessions[0].id(),
        serde_json::from_value(args).unwrap(),
        rho_tools::DEFAULT_MAX_OUTPUT_BYTES,
        &CancellationToken::new()
    )
    .is_err());
}

// Covers: Unicode paging and error evidence must survive compaction envelopes;
// offsets need to expand the actual match rather than the start of a huge log.
#[test]
fn focused_read_preserves_errors_and_expands_unicode_match_windows() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let text = format!("{} E0308 {}\0tail", "é🚀 ".repeat(200), "尾".repeat(800));
    let record = json!({"type":"node","transition":{"snapshot":{"private":"envelopesecret"}},
        "display_messages":[{"message":{"ToolResult":{"id":"call","ok":false,"content":text}}}]});
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(session.path())
            .unwrap(),
        "{record}"
    )
    .unwrap();
    let found = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"E0308"}),
    );
    let group = &found["sessions"][0];
    let excerpt = &group["excerpts"][0];
    let mut args =
        json!({"action":"read","session":group["session"],"anchor":excerpt["anchor"],"chars":317});
    let mut reconstructed = String::new();
    loop {
        let page = run(root.path(), cwd.path(), "current", args.clone());
        assert_eq!(page["role"], "tool_error");
        reconstructed.push_str(page["text"].as_str().unwrap());
        if page["next_start"].is_null() {
            break;
        }
        args["start"] = page["next_start"].clone();
    }
    assert_eq!(reconstructed, text);
    assert_eq!(
        excerpt["text"],
        text.chars()
            .skip(excerpt["start"].as_u64().unwrap() as usize)
            .take(320)
            .collect::<String>()
    );
    assert_eq!(
        ids(&run(
            root.path(),
            cwd.path(),
            "current",
            json!({"action":"search","query":"envelopesecret"})
        )),
        Vec::<&str>::new()
    );
}

// Covers: format-specific display evidence must not accidentally index model
// replacements, hidden reasoning or provider state. One table owns extraction.
#[test]
fn display_record_formats_preserve_roles_and_visible_omissions() {
    for kind in ["node", "snapshot", "snapshot_delta"] {
        let record = json!({"type":kind,"display_messages":[{"message":{"EnrichedAssistant":{
            "content":[{"Text":"answer"},{"Thinking":"secret"}],"provider_context":["secret"]}}}]});
        assert_eq!(
            search_evidence::extract(&record, 16)
                .into_iter()
                .map(|evidence| (evidence.role, evidence.text, evidence.omitted_blocks))
                .collect::<Vec<_>>(),
            vec![("assistant".into(), "answer".into(), 1)]
        );
    }
    for kind in ["session", "replace_history", "set_leaf", "upgrade"] {
        assert_eq!(
            search_evidence::extract(
                &json!({"type":kind,"messages":[{"User":[{"Text":"secret"}]}]}),
                0
            ),
            vec![]
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_transcripts_cannot_disclose_files_outside_session_storage() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("outsideneedle"))
        .unwrap();
    let outside = cwd.path().join("private.jsonl");
    fs::rename(session.path(), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, session.path()).unwrap();
    let result = run(
        root.path(),
        cwd.path(),
        "current",
        json!({"action":"search","query":"outsideneedle","scope":"all"}),
    );
    assert_eq!(
        (ids(&result), result["index"]["skipped_files"].clone()),
        (vec![], json!(1))
    );
}
