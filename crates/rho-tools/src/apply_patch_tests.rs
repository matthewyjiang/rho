use pretty_assertions::assert_eq;
use tempfile::TempDir;

use std::sync::{Arc, Mutex};

use super::apply::{apply_hunks_with_faults, rollback_one, FileChange};
use super::*;
use crate::{
    file_mutation::{
        AtomicCreateFaultInjector, AtomicInstallMethod, RewriteAttempt, RewriteFaultInjector,
    },
    tool::{ToolContext, ToolError},
    tool_card::{DiffRow, DiffRowKind},
};

fn test_context() -> (TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        cwd: dir.path().to_path_buf(),
        max_output_bytes: 12_000,
    };
    (dir, ctx)
}

fn message(error: ToolError) -> String {
    let ToolError::Message(message) = error else {
        panic!("expected ToolError::Message, got {error:?}");
    };
    message
}

async fn apply(
    input: &str,
    ctx: &ToolContext,
) -> Result<crate::file_mutation::FileMutationOutcome, ToolError> {
    let hunks = parse_patch(input).map_err(|error| ToolError::Message(error.to_string()))?;
    apply_hunks(
        hunks,
        |path| Ok(ctx.cwd.join(path)),
        str::to_string,
        ctx.max_output_bytes,
    )
    .await
}

async fn apply_with_rewrite_fault(
    input: &str,
    ctx: &ToolContext,
    fault: Arc<dyn RewriteFaultInjector>,
) -> Result<crate::file_mutation::FileMutationOutcome, ToolError> {
    let hunks = parse_patch(input).map_err(|error| ToolError::Message(error.to_string()))?;
    apply_hunks_with_faults(
        hunks,
        {
            let cwd = ctx.cwd.clone();
            move |path| Ok(cwd.join(path))
        },
        str::to_string,
        ctx.max_output_bytes,
        Some(fault),
        None,
    )
    .await
}

enum RewriteFaultPlan {
    ReplacementOnly,
    ReplacementAndRestoration,
}

impl RewriteFaultInjector for RewriteFaultPlan {
    fn fail_after_truncate(&self, attempt: RewriteAttempt) -> Option<std::io::Error> {
        let should_fail = match (self, attempt) {
            (Self::ReplacementOnly, RewriteAttempt::Replacement)
            | (Self::ReplacementAndRestoration, _) => true,
            (Self::ReplacementOnly, RewriteAttempt::Restoration) => false,
        };
        should_fail.then(|| std::io::Error::other(format!("injected {attempt:?} write failure")))
    }
}

struct RewriteFaultWithConcurrentEntry {
    path: std::path::PathBuf,
}

impl RewriteFaultInjector for RewriteFaultWithConcurrentEntry {
    fn fail_after_truncate(&self, attempt: RewriteAttempt) -> Option<std::io::Error> {
        if attempt == RewriteAttempt::Replacement {
            std::fs::write(&self.path, "concurrent\n").unwrap();
            Some(std::io::Error::other("injected replacement failure"))
        } else {
            None
        }
    }
}

struct FailBeforeStaging;

impl AtomicCreateFaultInjector for FailBeforeStaging {
    fn fail_before_staging(&self, display_path: &str) -> Option<std::io::Error> {
        Some(std::io::Error::other(format!(
            "injected staging failure for {display_path}"
        )))
    }
}

#[derive(Default)]
struct FailHardLinkStagingRemoval {
    staged: Mutex<Option<std::path::PathBuf>>,
}

impl AtomicCreateFaultInjector for FailHardLinkStagingRemoval {
    fn install_method(&self, _display_path: &str) -> AtomicInstallMethod {
        AtomicInstallMethod::HardLink
    }

    fn fail_staged_removal_after_hard_link(
        &self,
        staged: &std::path::Path,
    ) -> Option<std::io::Error> {
        *self.staged.lock().unwrap() = Some(staged.to_path_buf());
        Some(std::io::Error::other(
            "injected hard-link staging cleanup failure",
        ))
    }
}

async fn apply_with_create_fault(
    input: &str,
    ctx: &ToolContext,
    fault: Arc<dyn AtomicCreateFaultInjector>,
) -> Result<crate::file_mutation::FileMutationOutcome, ToolError> {
    let hunks = parse_patch(input).map_err(|error| ToolError::Message(error.to_string()))?;
    apply_hunks_with_faults(
        hunks,
        {
            let cwd = ctx.cwd.clone();
            move |path| Ok(cwd.join(path))
        },
        str::to_string,
        ctx.max_output_bytes,
        None,
        Some(fault),
    )
    .await
}

// Covers: a partial rewrite restores the target and rolls back nested add and move entries.
// Owner: apply_patch transaction/filesystem commit
#[tokio::test]
async fn replacement_write_failure_restores_full_filesystem_shape() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("target.txt"), "before\n").unwrap();
    std::fs::write(ctx.cwd.join("source.txt"), "source\n").unwrap();

    let error = apply_with_rewrite_fault(
        "*** Begin Patch\n*** Add File: created/inside/added.txt\n+added\n*** Update File: source.txt\n*** Move to: moved/inside/source.txt\n@@\n-source\n+moved\n*** Update File: target.txt\n@@\n-before\n+after\n*** End Patch",
        &ctx,
        Arc::new(RewriteFaultPlan::ReplacementOnly),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "could not write target.txt: injected Replacement write failure; original contents were restored; applied changes were rolled back"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("target.txt")).unwrap(),
        "before\n"
    );
    assert!(!ctx.cwd.join("created").exists());
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("source.txt")).unwrap(),
        "source\n"
    );
    assert!(!ctx.cwd.join("moved").exists());
}

// Covers: rollback preserves concurrent directory contents and reports the owned tree as dirty.
// Owner: apply_patch transaction/filesystem commit
#[tokio::test]
async fn rollback_reports_created_directory_kept_for_concurrent_contents() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("target.txt"), "before\n").unwrap();
    let concurrent = ctx.cwd.join("created/inside/concurrent.txt");

    let error = apply_with_rewrite_fault(
        "*** Begin Patch\n*** Add File: created/inside/added.txt\n+added\n*** Update File: target.txt\n@@\n-before\n+after\n*** End Patch",
        &ctx,
        Arc::new(RewriteFaultWithConcurrentEntry {
            path: concurrent.clone(),
        }),
    )
    .await
    .unwrap_err();
    let error = message(error);

    assert!(error.contains("rollback incomplete; unrecovered paths:"));
    assert!(error.contains("created/inside"));
    assert_eq!(std::fs::read_to_string(concurrent).unwrap(), "concurrent\n");
    assert!(!ctx.cwd.join("created/inside/added.txt").exists());
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("target.txt")).unwrap(),
        "before\n"
    );
}

// Covers: failed restoration reports the dirty target and never claims full rollback.
// Owner: apply_patch transaction/filesystem commit
#[tokio::test]
async fn restoration_failure_reports_unrecovered_dirty_path() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("target.txt"), "before\n").unwrap();

    let error = apply_with_rewrite_fault(
        "*** Begin Patch\n*** Add File: added.txt\n+added\n*** Update File: target.txt\n@@\n-before\n+after\n*** End Patch",
        &ctx,
        Arc::new(RewriteFaultPlan::ReplacementAndRestoration),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "could not write target.txt: injected Replacement write failure; failed to restore original contents: could not write target.txt: injected Restoration write failure; other applied changes were rolled back; rollback incomplete; unrecovered paths: target.txt"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("target.txt")).unwrap(),
        ""
    );
    assert!(!ctx.cwd.join("added.txt").exists());
}

// Covers: staging failure after nested parent creation leaves no filesystem entries.
// Owner: apply_patch transaction/filesystem commit
#[tokio::test]
async fn staging_failure_removes_owned_parent_directories() {
    let (_dir, ctx) = test_context();

    let error = apply_with_create_fault(
        "*** Begin Patch\n*** Add File: created/inside/added.txt\n+added\n*** End Patch",
        &ctx,
        Arc::new(FailBeforeStaging),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "failed to stage created/inside/added.txt: injected staging failure for created/inside/added.txt; applied changes were rolled back"
    );
    assert!(!ctx.cwd.join("created").exists());
}

// Covers: a zero-effect create failure reports only the staging error.
// Owner: apply_patch transaction/filesystem commit
#[tokio::test]
async fn staging_failure_under_existing_parent_does_not_claim_rollback() {
    let (_dir, ctx) = test_context();
    std::fs::create_dir(ctx.cwd.join("existing")).unwrap();

    let error = apply_with_create_fault(
        "*** Begin Patch\n*** Add File: existing/added.txt\n+added\n*** End Patch",
        &ctx,
        Arc::new(FailBeforeStaging),
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "failed to stage existing/added.txt: injected staging failure for existing/added.txt"
    );
    assert_eq!(
        std::fs::read_dir(ctx.cwd.join("existing")).unwrap().count(),
        0
    );
}

// Covers: hard-link installation followed by staging cleanup failure rolls back both links.
// Owner: apply_patch transaction/filesystem commit
#[tokio::test]
async fn hard_link_staging_cleanup_failure_is_tracked_and_rolled_back() {
    let (_dir, ctx) = test_context();
    let fault = Arc::new(FailHardLinkStagingRemoval::default());

    let error = apply_with_create_fault(
        "*** Begin Patch\n*** Add File: added.txt\n+added\n*** End Patch",
        &ctx,
        fault.clone(),
    )
    .await
    .unwrap_err();
    let staged = fault.staged.lock().unwrap().clone().unwrap();

    assert_eq!(
        message(error),
        format!(
            "failed to finalize added.txt: target was installed but staged hard link {} could not be removed: injected hard-link staging cleanup failure; applied changes were rolled back",
            staged.display()
        )
    );
    assert!(!ctx.cwd.join("added.txt").exists());
    assert!(!staged.exists());
    assert_eq!(std::fs::read_dir(&ctx.cwd).unwrap().count(), 0);
}

// Covers: malformed wrappers, empty updates, and invalid add bodies fail in the parser.
// Owner: apply_patch parser
#[test]
fn rejects_malformed_patch_documents() {
    let cases = [
        (
            "missing begin marker",
            "*** Add File: a.txt\n+x\n*** End Patch",
        ),
        (
            "missing end marker",
            "*** Begin Patch\n*** Add File: a.txt\n+x",
        ),
        (
            "empty update",
            "*** Begin Patch\n*** Update File: a.txt\n*** End Patch",
        ),
        (
            "add line without plus",
            "*** Begin Patch\n*** Add File: a.txt\nplain\n*** End Patch",
        ),
        (
            "heredoc wrapper",
            "<<EOF\n*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch\nEOF",
        ),
    ];

    for (name, input) in cases {
        assert!(parse_patch(input).is_err(), "case should fail: {name}");
    }
}

// Covers: whitespace-only context keeps all content after the required prefix.
// Owner: apply_patch parser
#[test]
fn parses_whitespace_only_context_lines() {
    let hunks =
        parse_patch("*** Begin Patch\n*** Update File: a.txt\n@@\n   \n-old\n+new\n*** End Patch")
            .unwrap();
    let Hunk::Update { chunks, .. } = &hunks[0] else {
        panic!("expected update hunk");
    };
    assert_eq!(chunks[0].old_lines, ["  ", "old"]);
    assert_eq!(chunks[0].new_lines, ["  ", "new"]);
}

// Covers: reverse chunk contexts fail as out of order without changing the file.
// Owner: apply_patch content derivation
#[tokio::test]
async fn rejects_reverse_chunk_contexts_without_mutation() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("ordered.txt");
    let original = "first\nsecond\nthird\nfourth\n";
    std::fs::write(&path, original).unwrap();

    let error = apply(
        "*** Begin Patch\n*** Update File: ordered.txt\n@@ third\n-fourth\n+FOURTH\n@@ first\n-second\n+SECOND\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "patch chunks overlap or apply out of order in ordered.txt"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
}

// Covers: repeated context headers continue searching forward for ordered chunks.
// Owner: apply_patch content derivation
#[tokio::test]
async fn applies_ordered_chunks_with_repeated_contexts() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("repeated.txt");
    std::fs::write(&path, "section\nold\nsection\nold\n").unwrap();

    apply(
        "*** Begin Patch\n*** Update File: repeated.txt\n@@ section\n-old\n+first\n@@ section\n-old\n+second\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "section\nfirst\nsection\nsecond\n"
    );
}

// Covers: deleting an unterminated final line preserves the preceding untouched CRLF.
// Owner: apply_patch content derivation
#[tokio::test]
async fn preserves_untouched_ending_before_deleted_final_line() {
    let (_dir, ctx) = test_context();
    let path = ctx.cwd.join("unterminated.txt");
    std::fs::write(&path, "one\r\ntwo").unwrap();

    apply(
        "*** Begin Patch\n*** Update File: unterminated.txt\n@@\n-two\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(path).unwrap(), b"one\r\n");
}

// Covers: one transaction applies add/update/delete and returns current metadata/output shapes.
// Owner: apply_patch application
#[tokio::test]
async fn applies_mixed_operations_with_numbered_snapshots() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("modify.txt"), "line1\nline2\n").unwrap();
    std::fs::write(ctx.cwd.join("delete.txt"), "obsolete\n").unwrap();

    let outcome = apply(
        "*** Begin Patch\n*** Add File: nested/new.txt\n+created\n*** Delete File: delete.txt\n*** Update File: modify.txt\n@@\n-line2\n+changed\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("nested/new.txt")).unwrap(),
        "created\n"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("modify.txt")).unwrap(),
        "line1\nchanged\n"
    );
    assert!(!ctx.cwd.join("delete.txt").exists());
    assert_eq!(
        outcome.display_paths,
        vec!["nested/new.txt", "delete.txt", "modify.txt"]
    );
    assert!(outcome.content.contains("nested/new.txt"));
    assert!(outcome.content.contains("modify.txt"));
    assert!(!outcome.content.contains("[nested/new.txt#"));
    assert!(!outcome.content.contains("[modify.txt#"));
    assert!(!outcome.content.contains("@@"));
    assert!(outcome.diff.contains("--- a/modify.txt"));
}

// Covers: a move reports and mutates both its source and destination paths.
// Owner: apply_patch application
#[tokio::test]
async fn moves_report_both_affected_paths() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("before.txt"), "old\n").unwrap();

    let outcome = apply(
        "*** Begin Patch\n*** Update File: before.txt\n*** Move to: after.txt\n@@\n-old\n+new\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(outcome.display_paths, ["before.txt", "after.txt"]);
    assert!(!ctx.cwd.join("before.txt").exists());
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("after.txt")).unwrap(),
        "new\n"
    );
}

// Covers: workspace escape paths and move clobbers fail without mutation.
// Owner: apply_patch application policy
#[tokio::test]
async fn rejects_unsafe_paths_and_existing_move_destination() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("src.txt"), "source\n").unwrap();
    std::fs::write(ctx.cwd.join("dst.txt"), "existing\n").unwrap();

    let escape = apply(
        "*** Begin Patch\n*** Add File: ../escape.txt\n+nope\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(escape),
        "patch path must not contain '..': ../escape.txt"
    );

    let clobber = apply(
        "*** Begin Patch\n*** Update File: src.txt\n*** Move to: dst.txt\n@@\n-source\n+moved\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(clobber),
        "Refusing to move to 'dst.txt': destination already exists"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("src.txt")).unwrap(),
        "source\n"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("dst.txt")).unwrap(),
        "existing\n"
    );
}

// Covers: Add File never overwrites an existing target.
// Owner: apply_patch transaction planning
#[tokio::test]
async fn rejects_add_for_an_existing_file() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("existing.txt"), "keep\n").unwrap();

    let error = apply(
        "*** Begin Patch\n*** Add File: existing.txt\n+replace\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap_err();

    assert_eq!(
        message(error),
        "Refusing to add 'existing.txt': file already exists"
    );
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("existing.txt")).unwrap(),
        "keep\n"
    );
}

// Covers: update hunks retain untouched mixed line endings and use the preferred ending for replacements.
// Owner: apply_patch content derivation
#[tokio::test]
async fn preserves_untouched_line_endings_during_mixed_eol_update() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("mixed.txt"), "one\r\ntwo\nthree\r\n").unwrap();

    apply(
        "*** Begin Patch\n*** Update File: mixed.txt\n@@\n-three\n+changed\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(ctx.cwd.join("mixed.txt")).unwrap(),
        b"one\r\ntwo\nchanged\r\n"
    );
}

// Covers: rollback refuses to overwrite a file changed after this patch's write.
// Owner: apply_patch transaction rollback
#[tokio::test]
async fn rollback_preserves_a_concurrent_external_change() {
    let (_dir, ctx) = test_context();
    let target = ctx.cwd.join("shared.txt");
    std::fs::write(&target, "external\n").unwrap();
    let permissions = std::fs::metadata(&target).unwrap().permissions();
    let change = FileChange::Update {
        target: target.clone(),
        display_path: "shared.txt".into(),
        old_content: "before\n".into(),
        new_content: "ours\n".into(),
        permissions,
        move_from: None,
    };

    rollback_one(&change).await.unwrap_err();
    assert_eq!(std::fs::read_to_string(target).unwrap(), "external\n");
}

#[cfg(unix)]
// Covers: move preserves executable mode and delete rejects a symlink leaf.
// Owner: apply_patch filesystem entry mutation
#[tokio::test]
async fn move_preserves_mode_and_delete_rejects_symlink_leaf() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};

    let (_dir, ctx) = test_context();
    let source = ctx.cwd.join("script.sh");
    std::fs::write(&source, "echo old\n").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
    apply(
        "*** Begin Patch\n*** Update File: script.sh\n*** Move to: moved.sh\n@@\n-echo old\n+echo new\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::metadata(ctx.cwd.join("moved.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );

    symlink(ctx.cwd.join("moved.sh"), ctx.cwd.join("alias.sh")).unwrap();
    let error = apply(
        "*** Begin Patch\n*** Delete File: alias.sh\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap_err();
    assert_eq!(
        message(error),
        "apply_patch cannot delete or move symlink path 'alias.sh'"
    );
    assert!(ctx.cwd.join("alias.sh").is_symlink());
    assert!(ctx.cwd.join("moved.sh").is_file());
}

// Covers: path conflicts are detected before any operation commits.
// Owner: apply_patch transaction planning
#[tokio::test]
async fn rejects_delete_and_move_of_same_source() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("a.txt"), "body\n").unwrap();

    apply(
        "*** Begin Patch\n*** Delete File: a.txt\n*** Update File: a.txt\n*** Move to: b.txt\n@@\n-body\n+body\n*** End Patch",
        &ctx,
    )
    .await
    .unwrap_err();
    assert_eq!(
        std::fs::read_to_string(ctx.cwd.join("a.txt")).unwrap(),
        "body\n"
    );
    assert!(!ctx.cwd.join("b.txt").exists());
}

// Covers: a source changed after planning aborts the whole commit.
// Owner: apply_patch transaction commit
#[tokio::test]
async fn fails_closed_when_a_planned_file_changes() {
    let (_dir, ctx) = test_context();
    std::fs::write(ctx.cwd.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(ctx.cwd.join("b.txt"), "beta\n").unwrap();
    let cwd = ctx.cwd.clone();
    let hunks = parse_patch(
        "*** Begin Patch\n*** Update File: a.txt\n@@\n-alpha\n+ALPHA\n*** Update File: b.txt\n@@\n-beta\n+BETA\n*** End Patch",
    )
    .unwrap();

    apply_hunks(
        hunks,
        {
            let cwd = cwd.clone();
            move |path| {
                if path == "b.txt" {
                    std::fs::write(cwd.join("a.txt"), "tampered\n").unwrap();
                }
                Ok(cwd.join(path))
            }
        },
        str::to_string,
        ctx.max_output_bytes,
    )
    .await
    .unwrap_err();
    assert_eq!(
        std::fs::read_to_string(cwd.join("a.txt")).unwrap(),
        "tampered\n"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("b.txt")).unwrap(),
        "beta\n"
    );
}

// Covers: streamed projection distinguishes incomplete lines and move destinations.
// Owner: apply_patch proposed diff parser
#[test]
fn projects_partial_and_moved_diffs_leniently() {
    let partial = proposed_diff_lenient(
        "*** Begin Patch\n*** Add File: new.txt\n+first\n+part",
        ProposedDiffTrailingLine::CompleteLinesOnly,
    );
    assert_eq!(
        partial,
        ProposedDiff {
            files: vec![ProposedDiffFile {
                operation: ProposedDiffOperation::Add,
                display_path: "new.txt".into(),
                source_path: None,
                destination_path: Some("new.txt".into()),
                rows: vec![DiffRow::new(DiffRowKind::Added, None, "first")],
                added_lines: Some(1),
                removed_lines: Some(0),
            }],
            truncated: false,
        }
    );

    let moved = proposed_diff_lenient(
        "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** End Patch\n",
        ProposedDiffTrailingLine::CompleteLinesOnly,
    );
    assert_eq!(moved.files[0].display_path, "new.txt");
    assert_eq!(moved.files[0].source_path.as_deref(), Some("old.txt"));
    assert_eq!(moved.files[0].destination_path.as_deref(), Some("new.txt"));
}

// Covers: presenter path extraction includes both sides of a move in document order.
// Owner: apply_patch path projection
#[test]
fn extracts_affected_paths_including_moves() {
    assert_eq!(
        patch_paths_lenient(
            "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n-old\n+new\n*** Delete File: gone.txt\n*** End Patch"
        ),
        vec!["a.txt", "old.txt", "new.txt", "gone.txt"]
    );
    assert!(patch_paths_lenient("not a patch").is_empty());
}
