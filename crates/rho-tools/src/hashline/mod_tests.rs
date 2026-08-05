use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::tool::ToolContext;

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

// Covers: end-to-end apply must rewrite the target and return a chainable preview
// Owner: apply_prepared_sections (production mutation path)
#[tokio::test]
async fn edits_file_from_read_tag() {
    let dir = test_cwd();
    let original = "fn main() {\n    println!(\"hi\");\n}\n";
    let (path, tag) = write_sample(&dir, "sample.rs", original);
    let sections = vec![prepared(
        &dir,
        "sample.rs",
        &tag,
        "PUT 2.=2:\n+    println!(\"hello\");\n",
    )];

    let outcome = apply_prepared_sections(sections, 12_000).await.unwrap();

    let updated = std::fs::read_to_string(path).unwrap();
    assert_eq!(updated, "fn main() {\n    println!(\"hello\");\n}\n");
    let new_tag = compute_file_hash(&updated);
    assert!(
        outcome.content.contains(&format!("[sample.rs#{new_tag}]")),
        "{}",
        outcome.content
    );
    assert!(
        outcome.content.contains("2:    println!(\"hello\");"),
        "{}",
        outcome.content
    );
    assert!(
        outcome.content.contains("PUT 2:") || outcome.content.contains("PUT 2.=2:"),
        "expected ops summary with wire form: {}",
        outcome.content
    );
    assert!(
        !outcome.content.contains("@@"),
        "model content should not embed unified diff: {}",
        outcome.content
    );
    assert!(outcome.diff.contains("+    println!(\"hello\");"));
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

    let error = apply_prepared_sections(sections, 12_000).await.unwrap_err();

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

// Covers: multi-file documents must edit every section under one call
// Owner: apply_prepared_sections
#[tokio::test]
async fn edits_multiple_files_in_one_document() {
    let dir = test_cwd();
    let a = "one\n";
    let b = "two\n";
    let (a_path, a_tag) = write_sample(&dir, "a.txt", a);
    let (b_path, b_tag) = write_sample(&dir, "b.txt", b);
    let sections = vec![
        prepared(&dir, "a.txt", &a_tag, "PUT 1.=1:\n+ONE\n"),
        prepared(&dir, "b.txt", &b_tag, "PUT 1.=1:\n+TWO\n"),
    ];

    apply_prepared_sections(sections, 12_000).await.unwrap();

    assert_eq!(std::fs::read_to_string(a_path).unwrap(), "ONE\n");
    assert_eq!(std::fs::read_to_string(b_path).unwrap(), "TWO\n");
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

    // Plan can read b; commit cannot open it for rewrite.
    let mut perms = std::fs::metadata(&b_path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&b_path, perms).unwrap();

    let sections = vec![
        prepared(&dir, "a.txt", &a_tag, "PUT 1.=1:\n+ALPHA\n"),
        prepared(&dir, "b.txt", &b_tag, "PUT 1.=1:\n+BETA\n"),
    ];
    let err = match apply_prepared_sections(sections, 12_000).await {
        Ok(_) => panic!("expected multi-file commit failure"),
        Err(error) => error,
    };
    assert!(
        err.to_string().contains("rolled back") || err.to_string().contains("could not open"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(&a_path).unwrap(), a);

    // Cleanup readonly so tempdir can delete on Windows/unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&b_path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&b_path, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = std::fs::metadata(&b_path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        {
            perms.set_readonly(false);
        }
        std::fs::set_permissions(&b_path, perms).unwrap();
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

    let first = apply_prepared_sections(vec![prepared(&dir, "big.txt", &tag, &body)], 12_000)
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

// Covers: Edit.spec remains the schema owner for SDK registration
// Owner: edit tool surface
#[test]
fn edit_spec_stays_compact_and_named_edit() {
    let spec = Edit.spec();
    assert_eq!(spec.name, "edit");
    assert!(spec.description.len() < 1_800, "{}", spec.description.len());
    assert!(spec.description.contains("PUT 12:"));
    assert!(spec.description.contains("never `PUT 12.:`"));
}

// Covers: App Tool call path still works as a harness (not the product contract)
// Owner: App Tool harness
#[tokio::test]
async fn app_tool_harness_call_still_applies() {
    let dir = test_cwd();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 12_000,
    };
    let original = "a\nb\n";
    std::fs::write(dir.path().join("t.txt"), original).unwrap();
    let tag = compute_file_hash(original);
    let result = Edit
        .call(
            serde_json::json!({
                "input": format!("[t.txt#{tag}]\nPUT 1.=1:\n+A\n")
            }),
            ctx,
            "harness".into(),
        )
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("t.txt")).unwrap(),
        "A\nb\n"
    );
}
