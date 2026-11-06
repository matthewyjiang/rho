//! Finding saved sessions by id or id prefix within one directory.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::layout::{session_id_from_path, SessionUnit};

/// The first eight characters of a session id, as messages show it.
pub(super) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// No saved session in any workspace matches the requested id or prefix.
#[derive(Debug, thiserror::Error)]
#[error("no session found matching '{id_prefix}'")]
pub(crate) struct NoSessionFound {
    pub(super) id_prefix: String,
}

/// Narrows id-prefix matches to the sessions whose whole id is `id_prefix`
/// when there are any. Every id-prefix lookup goes through this, so a full
/// id is never ambiguous: ids are any string a client chose, and one can
/// prefix another.
pub(super) fn prefer_exact<T>(
    mut matches: Vec<T>,
    id_prefix: &str,
    id: impl Fn(&T) -> Option<String>,
) -> Vec<T> {
    if matches
        .iter()
        .any(|item| id(item).as_deref() == Some(id_prefix))
    {
        matches.retain(|item| id(item).as_deref() == Some(id_prefix));
    }
    matches
}

/// The id of a prefix match that holds its transcript path first.
pub(super) fn path_id<T>(item: &(PathBuf, T)) -> Option<String> {
    session_id_from_path(&item.0)
}

/// The one session among `matches` from a [`prefer_exact`] lookup.
pub(super) fn single_match<'a, T>(
    matches: &'a [T],
    id_prefix: &str,
) -> anyhow::Result<Option<&'a T>> {
    match matches {
        [] => Ok(None),
        [only] => Ok(Some(only)),
        _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
    }
}

pub(super) fn session_id(path: &Path) -> anyhow::Result<String> {
    session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))
}

pub(super) fn matching_session_files(dir: &Path, id_prefix: &str) -> anyhow::Result<Vec<PathBuf>> {
    let files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let unit = SessionUnit::from_path(&entry.path())?;
            let id = unit.id()?;
            id.starts_with(id_prefix).then(|| unit.transcript_path())
        })
        .collect::<Vec<_>>();
    let mut files = prefer_exact(files, id_prefix, |path| session_id_from_path(path));
    files.sort();
    Ok(files)
}
