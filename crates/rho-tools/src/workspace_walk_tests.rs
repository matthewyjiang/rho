use std::{
    ops::ControlFlow,
    time::{Duration, Instant},
};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{visit_files, HiddenFiles, WalkLimits, WalkOptions, WalkStop, WalkedFile};

fn options(hidden: HiddenFiles, max_entries: usize, deadline: Instant) -> WalkOptions {
    WalkOptions {
        hidden,
        limits: WalkLimits {
            max_entries,
            deadline,
        },
    }
}

fn collect(root: &std::path::Path, options: &WalkOptions) -> (WalkStop, Vec<String>) {
    let mut files = Vec::new();
    let stop = visit_files(root, options, |file: WalkedFile| {
        files.push(file.relative);
        ControlFlow::Continue(())
    });
    files.sort();
    (stop, files)
}

#[test]
fn honors_gitignore_without_git_checkout() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(dir.path().join("kept.txt"), "keep").unwrap();
    std::fs::write(dir.path().join("ignored.txt"), "hide").unwrap();

    let (stop, files) = collect(
        dir.path(),
        &options(
            HiddenFiles::Skip,
            10_000,
            Instant::now() + Duration::from_secs(5),
        ),
    );
    assert_eq!(stop, WalkStop::Completed);
    assert_eq!(files, vec!["kept.txt".to_string()]);
}

#[test]
fn skip_and_include_hidden_always_excludes_git() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".git/objects")).unwrap();
    std::fs::write(dir.path().join(".git/config"), "secret").unwrap();
    std::fs::write(dir.path().join(".hidden.txt"), "dot").unwrap();
    std::fs::write(dir.path().join("visible.txt"), "ok").unwrap();

    let (_, skipped) = collect(
        dir.path(),
        &options(
            HiddenFiles::Skip,
            10_000,
            Instant::now() + Duration::from_secs(5),
        ),
    );
    assert_eq!(skipped, vec!["visible.txt".to_string()]);

    let (_, included) = collect(
        dir.path(),
        &options(
            HiddenFiles::Include,
            10_000,
            Instant::now() + Duration::from_secs(5),
        ),
    );
    assert_eq!(
        included,
        vec![".hidden.txt".to_string(), "visible.txt".to_string()]
    );
}

#[cfg(unix)]
#[test]
fn never_yields_symlink_or_descends_symlink_directory() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    std::fs::write(dir.path().join("real.txt"), "real").unwrap();
    symlink(
        outside.path().join("secret.txt"),
        dir.path().join("link.txt"),
    )
    .unwrap();
    symlink(outside.path(), dir.path().join("escape")).unwrap();

    let (stop, files) = collect(
        dir.path(),
        &options(
            HiddenFiles::Include,
            10_000,
            Instant::now() + Duration::from_secs(5),
        ),
    );
    assert_eq!(stop, WalkStop::Completed);
    assert_eq!(files, vec!["real.txt".to_string()]);
}

#[test]
fn max_entries_returns_entry_limit() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("b.txt"), "b").unwrap();
    std::fs::write(dir.path().join("c.txt"), "c").unwrap();

    let (stop, _) = collect(
        dir.path(),
        &options(
            HiddenFiles::Skip,
            1,
            Instant::now() + Duration::from_secs(5),
        ),
    );
    assert_eq!(stop, WalkStop::EntryLimit);
}

#[test]
fn past_deadline_returns_deadline() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();

    let (stop, files) = collect(
        dir.path(),
        &options(
            HiddenFiles::Skip,
            10_000,
            Instant::now() - Duration::from_secs(1),
        ),
    );
    assert_eq!(stop, WalkStop::Deadline);
    assert!(files.is_empty());
}

#[test]
fn visitor_break_result_limit_stops_walk() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("b.txt"), "b").unwrap();

    let mut seen = 0usize;
    let stop = visit_files(
        dir.path(),
        &options(
            HiddenFiles::Skip,
            10_000,
            Instant::now() + Duration::from_secs(5),
        ),
        |_| {
            seen += 1;
            ControlFlow::Break(WalkStop::ResultLimit)
        },
    );
    assert_eq!(stop, WalkStop::ResultLimit);
    assert_eq!(seen, 1);
}
