use std::process::Command;

use pretty_assertions::assert_eq;

use super::*;

// Fixtures below are verbatim output of git 2.47 in scratch repos.

/// Mixed worktree: `a.txt` staged then edited again, `added.txt` new and
/// staged, binary `bin.dat` staged, `del.txt` staged deletion, `link` turned
/// into a symlink (type change), `old.txt` renamed with `git mv` and then
/// edited, and an untracked file whose path has a space and a glob character.
const MIXED_STATUS: &[u8] = b"# branch.oid 542a8d53d8a56c5e28be851801f21d0f589205a1\0\
    # branch.head main\0\
    1 MM N... 100644 100644 100644 78981922613b2afb6025042ff6bd878ac1994e85 9ad2ebbaff6f3397bb65002dcf4294d8d6243982 a.txt\0\
    1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 3e757656cf36eca53338e520d134963a44f793f8 added.txt\0\
    1 M. N... 100644 100644 100644 bdc955b7b2e610ad5a72302b139a2e6cb325519a 40d36f1cb4d063007c75d68c47b44fb356f164dd bin.dat\0\
    1 D. N... 100644 000000 000000 587be6b4c3f93f93c489c0111bba5596147a26cb 0000000000000000000000000000000000000000 del.txt\0\
    1 .T N... 100644 100644 120000 1f9d725a9de833a65966881dce2e907b86e72c5e 1f9d725a9de833a65966881dce2e907b86e72c5e link\0\
    2 RM N... 100644 100644 100644 4cb29ea38f70d7c61b2a3a25b02e3bdf44905402 4cb29ea38f70d7c61b2a3a25b02e3bdf44905402 R100 new [name].txt\0\
    old.txt\0\
    ? sub dir/n*.txt\0";

/// `git diff --cached --numstat -z -M` for the mixed worktree.
const MIXED_STAGED_NUMSTAT: &[u8] = b"1\t0\ta.txt\0\
    1\t0\tadded.txt\0\
    -\t-\tbin.dat\0\
    0\t1\tdel.txt\0\
    0\t0\t\0\
    old.txt\0\
    new [name].txt\0";

/// Detached HEAD with an intent-to-add file (`git add -N`) that git pairs
/// with a deleted tracked file as an unstaged-only rename.
const DETACHED_STATUS: &[u8] = b"# branch.oid d7c7adf1a4e973ac68a80dc8a4df229e191934fc\0\
    # branch.head (detached)\0\
    2 .R N... 100644 100644 100644 4cb29ea38f70d7c61b2a3a25b02e3bdf44905402 4cb29ea38f70d7c61b2a3a25b02e3bdf44905402 R100 moved c.txt\0\
    c.txt\0";

/// Before the first commit, with one staged file.
const UNBORN_STATUS: &[u8] = b"# branch.oid (initial)\0\
    # branch.head main\0\
    1 A. N... 000000 100644 100644 0000000000000000000000000000000000000000 45b983be36b73c0788dc9cbcb76cbb80fc7bb057 a.txt\0";

/// Both sides of a merge modified `c.txt`.
const CONFLICT_STATUS: &[u8] = b"# branch.oid 645edccfe1693672925d8eefffafb10e5852f7a7\0\
    # branch.head main\0\
    u UU N... 100644 100644 100644 100644 df967b96a579e45a18b8251732d16804b2e56a55 b19a1e93bec1317dc6097229e12afaffbfa74dc2 950b81b7eee953d050aa05a641f8e056c85dd1bd c.txt\0";

/// `git diff --numstat -z -M` for the conflict: one record against the
/// merge base, then one against "ours".
const CONFLICT_NUMSTAT: &[u8] = b"0\t0\tc.txt\0\
    4\t0\tc.txt\0";

fn changed(section: DiffSection, change: FileChange, path: &str) -> ChangedFile {
    ChangedFile {
        section,
        change,
        path: path.into(),
        orig_path: None,
        stats: None,
    }
}

// Covers: section split (one path in both Staged and Unstaged), status letter
// mapping, rename source pairing, paths with spaces, and branch detection.
// Owner: porcelain v2 parser.
#[test]
fn parse_status_groups_entries_by_section_in_git_order() {
    use super::DiffSection::{Staged, Unstaged, Untracked};

    let cases = [
        (
            "mixed worktree",
            MIXED_STATUS,
            Some("main"),
            vec![
                changed(Staged, FileChange::Modified, "a.txt"),
                changed(Staged, FileChange::Added, "added.txt"),
                changed(Staged, FileChange::Modified, "bin.dat"),
                changed(Staged, FileChange::Deleted, "del.txt"),
                ChangedFile {
                    orig_path: Some("old.txt".into()),
                    ..changed(Staged, FileChange::Renamed, "new [name].txt")
                },
                changed(Unstaged, FileChange::Modified, "a.txt"),
                changed(Unstaged, FileChange::TypeChanged, "link"),
                changed(Unstaged, FileChange::Modified, "new [name].txt"),
                changed(Untracked, FileChange::Untracked, "sub dir/n*.txt"),
            ],
        ),
        (
            "detached head with unstaged rename",
            DETACHED_STATUS,
            None,
            vec![ChangedFile {
                orig_path: Some("c.txt".into()),
                ..changed(Unstaged, FileChange::Renamed, "moved c.txt")
            }],
        ),
        (
            "unborn head",
            UNBORN_STATUS,
            Some("main"),
            vec![changed(Staged, FileChange::Added, "a.txt")],
        ),
        (
            "merge conflict",
            CONFLICT_STATUS,
            Some("main"),
            vec![changed(Unstaged, FileChange::Unmerged, "c.txt")],
        ),
        (
            "clean worktree",
            b"# branch.head main\0".as_slice(),
            Some("main"),
            vec![],
        ),
    ];

    for (name, bytes, branch, files) in cases {
        assert_eq!(
            parse_status(bytes).unwrap(),
            (branch.map(str::to_string), files),
            "{name}"
        );
    }
}

// Covers: binary files and renames (split across three records) get keyed by
// the new path, and unmerged paths keep the count against "ours".
// Owner: numstat parser.
#[test]
fn parse_numstat_keys_counts_by_new_path() {
    let cases: [(&str, &[u8], NumstatMap); 3] = [
        (
            "staged mixed worktree",
            MIXED_STAGED_NUMSTAT,
            HashMap::from([
                ("a.txt".into(), Some((1, 0))),
                ("added.txt".into(), Some((1, 0))),
                ("bin.dat".into(), None),
                ("del.txt".into(), Some((0, 1))),
                ("new [name].txt".into(), Some((0, 0))),
            ]),
        ),
        (
            "merge conflict",
            CONFLICT_NUMSTAT,
            HashMap::from([("c.txt".into(), Some((4, 0)))]),
        ),
        ("no changes", b"".as_slice(), HashMap::new()),
    ];

    for (name, bytes, expected) in cases {
        assert_eq!(parse_numstat(bytes).unwrap(), expected, "{name}");
    }
}

// Covers: truncated or unfamiliar output fails loudly instead of misparsing,
// e.g. a rename missing its source record or a record split on a bad field.
// Owner: porcelain v2 and numstat parsers.
#[test]
fn parsers_reject_malformed_records() {
    let status_cases: [&[u8]; 3] = [
        b"2 R. N... 100644 100644 100644 aaaa aaaa R100 new.txt\0",
        b"1 Z. N... 100644 100644 100644 aaaa aaaa a.txt\0",
        b"1 M. N... 100644\0",
    ];
    for bytes in status_cases {
        assert!(
            parse_status(bytes).is_err(),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }

    let numstat_cases: [&[u8]; 3] = [b"x\t0\ta.txt\0", b"1\t0\0", b"0\t0\t\0old.txt\0"];
    for bytes in numstat_cases {
        assert!(
            parse_numstat(bytes).is_err(),
            "{}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[cfg(unix)]
#[test]
fn command_output_keeps_first_stderr_line_only() {
    use std::os::unix::process::ExitStatusExt;

    let output = Output {
        // Raw wait status 256 is exit code 1.
        status: std::process::ExitStatus::from_raw(256),
        stdout: Vec::new(),
        stderr: b"fatal: not a git repository\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\n".to_vec(),
    };
    let error = command_output(output).unwrap_err();
    assert_eq!(error.to_string(), "fatal: not a git repository");
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=rho", "-c", "user.email=rho@example.com"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("git should be available for local diff tests");
    assert!(status.success(), "git {args:?} failed");
}

/// Patch headers and changed lines, which identify the diffed files and
/// sides without depending on hunk context.
fn patch_summary(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter(|line| {
            line.starts_with("diff --git ")
                || (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
        })
        .collect()
}

// Covers: listing from a subdirectory still covers the whole repo, counts join
// to the right section, and each patch runs at the repo root against only its
// own file (literal pathspecs, rename pairing, untracked `--no-index`).
// Owner: git command wiring for `/diff`.
#[test]
fn collect_status_and_file_patch_cover_whole_repo_from_subdirectory() {
    use super::DiffSection::{Staged, Unstaged, Untracked};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    std::fs::write(root.join("[a].txt"), "x\n").unwrap();
    std::fs::write(root.join("old.txt"), "one\ntwo\nthree\n").unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-q", "-m", "init"]);
    std::fs::write(root.join("a.txt"), "a\nb\n").unwrap();
    run_git(root, &["add", "a.txt"]);
    std::fs::write(root.join("a.txt"), "a\nb\nc\n").unwrap();
    // Unquoted, `[a].txt` is a glob that also matches `a.txt`.
    std::fs::write(root.join("[a].txt"), "y\n").unwrap();
    run_git(root, &["mv", "old.txt", "new name.txt"]);
    std::fs::create_dir(root.join("sub dir")).unwrap();
    std::fs::write(root.join("sub dir/new file.txt"), "hi\n").unwrap();

    let status = collect_status(&root.join("sub dir")).unwrap();

    assert_eq!(
        status.repo_root.canonicalize().unwrap(),
        root.canonicalize().unwrap()
    );
    let expected_files = vec![
        ChangedFile {
            stats: Some((1, 0)),
            ..changed(Staged, FileChange::Modified, "a.txt")
        },
        ChangedFile {
            orig_path: Some("old.txt".into()),
            stats: Some((0, 0)),
            ..changed(Staged, FileChange::Renamed, "new name.txt")
        },
        ChangedFile {
            stats: Some((1, 1)),
            ..changed(Unstaged, FileChange::Modified, "[a].txt")
        },
        ChangedFile {
            stats: Some((1, 0)),
            ..changed(Unstaged, FileChange::Modified, "a.txt")
        },
        changed(Untracked, FileChange::Untracked, "sub dir/new file.txt"),
    ];
    assert_eq!(
        (status.branch.as_deref(), &status.files),
        (Some("main"), &expected_files)
    );

    let patches: Vec<String> = status
        .files
        .iter()
        .map(|file| file_patch(&status.repo_root, file).unwrap())
        .collect();
    let summaries: Vec<Vec<&str>> = patches.iter().map(|patch| patch_summary(patch)).collect();
    assert_eq!(
        summaries,
        vec![
            vec!["diff --git a/a.txt b/a.txt", "+b"],
            vec!["diff --git a/old.txt b/new name.txt"],
            vec!["diff --git a/[a].txt b/[a].txt", "-x", "+y"],
            vec!["diff --git a/a.txt b/a.txt", "+c"],
            vec![
                "diff --git a/sub dir/new file.txt b/sub dir/new file.txt",
                "+hi"
            ],
        ]
    );
}
