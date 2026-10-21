use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

fn test_cwd() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn write_sample(dir: &TempDir, name: &str, body: &str) -> (std::path::PathBuf, String) {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    (path, compute_file_hash(body))
}

fn prepared(dir: &TempDir, name: &str, tag: &str, ops_body: &str) -> PreparedSection {
    let input = format!("[{name}#{tag}]\n{ops_body}");
    let mut parsed = parse_hashline(&input).unwrap();
    PreparedSection {
        path: dir.path().join(name),
        display_path: name.into(),
        section: parsed.remove(0),
    }
}

// Covers: a second edit must succeed using only the first edit's returned preview
// Owner: apply_prepared_sections chain contract
#[tokio::test]
async fn chains_second_edit_from_post_edit_preview_without_reread() {
    let dir = test_cwd();
    let original = "alpha\nbeta\ngamma\n";
    let (path, tag) = write_sample(&dir, "chain.txt", original);
    let first = apply_prepared_sections(
        vec![prepared(&dir, "chain.txt", &tag, "PUT 2.=2:\n+BETA\n")],
        12_000,
        /*host_files*/ None,
    )
    .await
    .unwrap();

    let mid = std::fs::read_to_string(&path).unwrap();
    assert_eq!(mid, "alpha\nBETA\ngamma\n");
    let new_tag = compute_file_hash(&mid);
    assert!(
        first.content.contains(&format!("[chain.txt#{new_tag}]")),
        "{}",
        first.content
    );

    apply_prepared_sections(
        vec![prepared(&dir, "chain.txt", &new_tag, "PUT 3.=3:\n+GAMMA\n")],
        12_000,
        /*host_files*/ None,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "alpha\nBETA\nGAMMA\n"
    );
}

// Covers: stale tags must leave the file unchanged and return a live snapshot
// Owner: apply_prepared_sections
#[tokio::test]
async fn stale_tag_leaves_file_untouched() {
    let dir = test_cwd();
    let original = "alpha\nbeta\n";
    let (path, _) = write_sample(&dir, "sample.rs", original);
    let sections = vec![prepared(&dir, "sample.rs", "DEAD", "PUT 1.=1:\n+nope\n")];

    let error = apply_prepared_sections(sections, 12_000, /*host_files*/ None)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("tag mismatch"), "{error}");
    assert!(error.to_string().contains("Live snapshot"), "{error}");
    let live_tag = compute_file_hash(original);
    assert!(
        error
            .to_string()
            .contains(&format!("[sample.rs#{live_tag}]")),
        "{error}"
    );
    assert!(error.to_string().contains("1:alpha"), "{error}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

// Covers: multi-file commit failure must rollback earlier writes
// Owner: apply_prepared_sections atomicity
#[tokio::test]
async fn multi_file_rollback_restores_earlier_writes() {
    let dir = test_cwd();
    let a = "alpha\n";
    let b = "beta\n";
    let (a_path, a_tag) = write_sample(&dir, "a.txt", a);
    let (b_path, b_tag) = write_sample(&dir, "b.txt", b);

    // Make b unwritable so plan can still read it while commit cannot open it
    // for rewrite. Exclusive locks are mandatory on Windows and would also
    // block the plan-phase read, so they cannot isolate a commit-path failure.
    // Restore original mode on drop so TempDir cleanup succeeds on Windows and
    // so we avoid clippy::permissions_set_readonly_false (world-writable on Unix).
    struct RestorePerms {
        path: std::path::PathBuf,
        perms: std::fs::Permissions,
    }
    impl Drop for RestorePerms {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.path, self.perms.clone());
        }
    }
    let original_b_perms = std::fs::metadata(&b_path).unwrap().permissions();
    let mut b_perms = original_b_perms.clone();
    b_perms.set_readonly(true);
    std::fs::set_permissions(&b_path, b_perms).unwrap();
    let _restore = RestorePerms {
        path: b_path.clone(),
        perms: original_b_perms,
    };

    let sections = vec![
        prepared(&dir, "a.txt", &a_tag, "PUT 1.=1:\n+ALPHA\n"),
        prepared(&dir, "b.txt", &b_tag, "PUT 1.=1:\n+BETA\n"),
    ];
    let err = match apply_prepared_sections(sections, 12_000, /*host_files*/ None).await {
        Ok(_) => panic!("expected multi-file commit failure"),
        Err(error) => error,
    };

    assert!(
        err.to_string().contains("rolled back"),
        "expected earlier writes rolled back after b commit failure: {err}"
    );
    assert_eq!(std::fs::read_to_string(&a_path).unwrap(), a);
}

// Covers: a host-served file is tagged, matched, and committed against the
// host's text (an unsaved buffer) and never written on disk, while an
// unserved file in the same document still commits on disk.
// Owner: host_files routing, through apply_prepared_sections
#[tokio::test]
async fn host_served_paths_edit_the_host_text_and_leave_disk_alone() {
    use std::sync::Arc;

    use crate::host_files::{test_support::MemoryHost, HostTextFiles};

    let dir = test_cwd();
    std::fs::create_dir_all(dir.path().join("served")).unwrap();
    let (served, _) = write_sample(&dir, "served/a.txt", "on disk\n");
    let (unserved, unserved_tag) = write_sample(&dir, "b.txt", "beta\n");
    let host = Arc::new(MemoryHost::new(
        &dir.path().join("served"),
        &[(&served, "in buffer\n")],
    ));
    let host_files: Arc<dyn HostTextFiles> = host.clone();

    apply_prepared_sections(
        vec![
            prepared(
                &dir,
                "served/a.txt",
                &compute_file_hash("in buffer\n"),
                "PUT 1.=1:\n+edited buffer\n",
            ),
            prepared(&dir, "b.txt", &unserved_tag, "PUT 1.=1:\n+BETA\n"),
        ],
        12_000,
        Some(&host_files),
    )
    .await
    .unwrap();

    assert_eq!(
        (
            host.writes.lock().unwrap().clone(),
            std::fs::read_to_string(&served).unwrap(),
            std::fs::read_to_string(&unserved).unwrap(),
        ),
        (
            vec![(served.clone(), "edited buffer\n".to_string())],
            "on disk\n".to_string(),
            "BETA\n".to_string(),
        )
    );
}

// Covers: when a host formats what it saves, a later file's failure rolls an
// earlier host file back from the text the host holds, not the text sent.
// Owner: host_files routing, through apply_prepared_sections rollback
#[tokio::test]
async fn rollback_restores_a_host_file_the_host_reformatted() {
    use std::sync::Arc;

    use crate::host_files::{test_support::MemoryHost, HostTextFiles};

    let dir = test_cwd();
    let (first, _) = write_sample(&dir, "a.txt", "");
    let (second, _) = write_sample(&dir, "b.txt", "");
    let mut host = MemoryHost::new(dir.path(), &[(&first, "alpha\n"), (&second, "beta\n")]);
    host.format = str::to_uppercase;
    host.refused = Some(second.clone());
    let host = Arc::new(host);
    let host_files: Arc<dyn HostTextFiles> = host.clone();

    let error = apply_prepared_sections(
        vec![
            prepared(
                &dir,
                "a.txt",
                &compute_file_hash("alpha\n"),
                "PUT 1.=1:\n+edited\n",
            ),
            prepared(
                &dir,
                "b.txt",
                &compute_file_hash("beta\n"),
                "PUT 1.=1:\n+edited\n",
            ),
        ],
        12_000,
        Some(&host_files),
    )
    .await
    .unwrap_err();

    assert_eq!(
        (
            error
                .to_string()
                .ends_with("applied changes were rolled back"),
            host.files.lock().unwrap().get(&first).cloned(),
        ),
        (true, Some("ALPHA\n".to_string()))
    );
}

// Covers: a multi-file edit cancelled while a later file's write is in
// flight still settles: that write's failure rolls back the earlier commit
// instead of leaving it applied.
// Owner: apply_prepared_sections commit, through host_files
#[tokio::test]
async fn a_cancelled_edit_still_rolls_back() {
    use std::sync::Arc;

    use tokio::sync::Notify;

    use crate::host_files::{
        test_support::{Hold, MemoryHost},
        HostTextFiles,
    };

    let dir = test_cwd();
    let (first, _) = write_sample(&dir, "a.txt", "");
    let (second, _) = write_sample(&dir, "b.txt", "");
    let (held, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let mut host = MemoryHost::new(dir.path(), &[(&first, "alpha\n"), (&second, "beta\n")]);
    host.refused = Some(second.clone());
    host.hold = Some(Hold {
        path: second.clone(),
        held: Arc::clone(&held),
        release: Arc::clone(&release),
        used: Default::default(),
    });
    let host = Arc::new(host);
    let mut applied = host.applied.subscribe();
    let host_files: Arc<dyn HostTextFiles> = host.clone();

    {
        let edit = apply_prepared_sections(
            vec![
                prepared(
                    &dir,
                    "a.txt",
                    &compute_file_hash("alpha\n"),
                    "PUT 1.=1:\n+edited\n",
                ),
                prepared(
                    &dir,
                    "b.txt",
                    &compute_file_hash("beta\n"),
                    "PUT 1.=1:\n+edited\n",
                ),
            ],
            12_000,
            Some(&host_files),
        );
        tokio::pin!(edit);
        tokio::select! {
            biased;
            () = held.notified() => {}
            _ = &mut edit => panic!("the edit finished while a write was held"),
        }
        // Dropping the edit here cancels the call.
    }
    release.notify_one();
    // The first commit and its rollback; a timeout means the rollback never ran.
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        applied.wait_for(|count| *count == 2),
    )
    .await
    .expect("the cancelled edit rolls back")
    .unwrap();

    assert_eq!(
        host.files.lock().unwrap().get(&first).cloned(),
        Some("alpha\n".to_string())
    );
}

// Covers: after a host reformats a committed file, the preview focuses on
// the span its saved text changed: changed, inserted, removed, or none.
// Owner: hashline post-commit preview focus
#[test]
fn saved_text_focus_is_the_changed_span() {
    use super::execute::changed_span;

    let cases = [
        ("a\nb\nc\n", "a\nB\nC\n", vec![2, 3]),
        ("a\nc\n", "a\nb\nc\n", vec![2, 2]),
        ("a\nb\nc\n", "a\nc\n", vec![2]),
        ("a\n", "a\n", vec![]),
    ];
    for (original, updated, expected) in cases {
        assert_eq!(changed_span(original, updated), expected, "{updated:?}");
    }
}

// Covers: structural PUT omits chainable body; follow-up CUT uses a fresh full
// read tag (gold: large rewrite then cleanup leaves a clean file)
// Owner: apply_prepared_sections structural chain contract
#[tokio::test]
async fn structural_put_then_cut_cleanup_from_fresh_read() {
    let dir = test_cwd();
    let mut original = String::new();
    for i in 1..=50 {
        original.push_str(&format!("line-{i}\n"));
    }
    // Plant dead block in the middle that a large PUT will introduce, then CUT.
    let (path, tag) = write_sample(&dir, "big.txt", &original);

    // Structural replace of lines 10..=49 with a short block that still has junk.
    let mut body = String::from("PUT 10.=49:\n");
    for i in 0..5 {
        body.push_str(&format!("+keep-{i}\n"));
    }
    body.push_str("+DEAD_START\n+dead-a\n+dead-b\n+DEAD_END\n");
    for i in 5..8 {
        body.push_str(&format!("+keep-{i}\n"));
    }

    let first = apply_prepared_sections(
        vec![prepared(&dir, "big.txt", &tag, &body)],
        12_000,
        /*host_files*/ None,
    )
    .await
    .unwrap();
    assert!(
        first.content.contains("structural edit"),
        "expected structural notice: {}",
        first.content
    );
    assert!(
        first.content.contains("no chainable body lines"),
        "{}",
        first.content
    );
    // Must not expose numbered anchors after structural rewrite.
    assert!(
        !first.content.contains("10:keep-0"),
        "structural result leaked body lines: {}",
        first.content
    );

    let mid = std::fs::read_to_string(&path).unwrap();
    let mid_tag = compute_file_hash(&mid);
    // Find DEAD block via the fresh full-file content (simulates re-read).
    let lines: Vec<&str> = mid.lines().collect();
    let start = lines.iter().position(|l| *l == "DEAD_START").unwrap() + 1;
    let end = lines.iter().position(|l| *l == "DEAD_END").unwrap() + 1;
    assert!(end - start + 1 >= 2);

    apply_prepared_sections(
        vec![prepared(
            &dir,
            "big.txt",
            &mid_tag,
            &format!("CUT {start}.={end}\n"),
        )],
        12_000,
        /*host_files*/ None,
    )
    .await
    .unwrap();

    let final_text = std::fs::read_to_string(path).unwrap();
    assert!(!final_text.contains("DEAD_START"));
    assert!(!final_text.contains("dead-a"));
    assert!(final_text.contains("keep-0"));
    assert!(final_text.contains("line-1"));
    assert!(final_text.contains("line-50") || final_text.contains("keep-7"));
}
