//! What a worktree takes on disk, as `rho worktrees status` reports it.

use std::{collections::HashSet, path::Path};

/// The bytes of every file under `path`: symlinks count as themselves and
/// are never followed, and a file linked more than once there, as Cargo
/// links each binary it builds, counts once. About what removing it frees,
/// give or take block rounding. Unreadable entries count as empty.
pub(crate) async fn size(path: &Path) -> u64 {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || walk(&path, &mut HashSet::new()))
        .await
        .unwrap_or_default()
}

/// A file's identity on its device, where the platform tells it.
type FileId = (u64, u64);

fn walk(dir: &Path, seen: &mut HashSet<FileId>) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut bytes = 0;
    for entry in entries.flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => bytes += walk(&entry.path(), seen),
            Ok(_) => {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if linked(&meta).is_none_or(|id| seen.insert(id)) {
                    bytes += meta.len();
                }
            }
            Err(_) => {}
        }
    }
    bytes
}

/// The identity of a file with more than one link, which a walk must count
/// once; `None` for one linked once, or where the platform cannot tell.
#[cfg(unix)]
fn linked(meta: &std::fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    (meta.nlink() > 1).then(|| (meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn linked(_meta: &std::fs::Metadata) -> Option<FileId> {
    None
}

#[cfg(test)]
#[path = "size_tests.rs"]
mod tests;
