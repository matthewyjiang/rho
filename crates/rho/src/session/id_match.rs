//! Finding saved sessions by id or id prefix within one directory.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::layout::{session_id_from_path, SessionUnit};

/// No saved session in any workspace matches the requested id or prefix.
#[derive(Debug, thiserror::Error)]
#[error("no session found matching '{id_prefix}'")]
pub(crate) struct NoSessionFound {
    pub(super) id_prefix: String,
}

/// The one session `id_prefix` names among `matches` (transcript path and
/// workspace pairs). A session whose whole id is `id_prefix` wins over ones
/// it is only a prefix of, so a full id is never ambiguous: ids are any
/// string a client chose, and one can prefix another.
pub(super) fn single_match<'a>(
    matches: &'a [(PathBuf, PathBuf)],
    id_prefix: &str,
) -> anyhow::Result<Option<&'a (PathBuf, PathBuf)>> {
    let exact = matches
        .iter()
        .filter(|(path, _)| session_id_from_path(path).as_deref() == Some(id_prefix))
        .collect::<Vec<_>>();
    match (exact.as_slice(), matches) {
        ([only], _) => Ok(Some(only)),
        ([], []) => Ok(None),
        ([], [only]) => Ok(Some(only)),
        _ => anyhow::bail!("multiple sessions match '{id_prefix}'; use a longer UUID prefix"),
    }
}

pub(super) fn session_id(path: &Path) -> anyhow::Result<String> {
    session_id_from_path(path)
        .ok_or_else(|| anyhow::anyhow!("session file has invalid name: {}", path.display()))
}

pub(super) fn matching_session_files(dir: &Path, id_prefix: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let unit = SessionUnit::from_path(&entry.path())?;
            let id = unit.id()?;
            id.starts_with(id_prefix).then(|| unit.transcript_path())
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}
