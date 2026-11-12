//! Rewrites the 2.x files on disk to the 3.0 layout, once.
//!
//! 3.0 loads `config.toml` strictly, so every rho process calls
//! [`upgrade_2x`] before its first load. [`migrate_2x`] decides what moves;
//! this module reads and writes the files. Each file it rewrites is first
//! copied beside itself as `<name>.2x.bak`, and `config.toml` is written
//! last, so an interrupted upgrade runs again from the same 2.x file and
//! ends in the same place. A value `tui.toml` or `clients/tui.toml` already
//! holds wins over the 2.x one. Keys neither version knows stay in
//! `config.toml`, where loading names them. Besides the stderr notice, the
//! upgrade leaves a note the next TUI shows once
//! ([`super::upgrade_2x_note`]).

use std::path::{Path, PathBuf};

use anyhow::Context;
use rho_providers::credentials::{CredentialStoreBackend, WebSearchCredential};
use toml_edit::{DocumentMut, Item};

use super::{
    document::{shown, ConfigDocument},
    migrate_2x::{migrate_2x, Change, MigratedFile},
};
use crate::config_writer::write_bytes_atomically;

/// What one upgrade did, for the startup notice and the TUI's note.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Upgraded {
    /// The 2.x `config.toml`, as it was.
    pub(crate) backup: PathBuf,
    /// What [`migrate_2x`] changed, in order.
    pub(crate) report: Vec<Change>,
    /// 2.x keys, as the file named them, whose 3.0 file or credential
    /// store already held a different value, which was kept.
    pub(crate) already_set: Vec<String>,
    /// Why the plaintext web-search keys did not reach the credential
    /// store. They are in [`Self::backup`] only.
    pub(crate) secrets_error: Option<String>,
}

/// Upgrades the 2.x `config_path`, writing the TUI's files and its note
/// under `rho_dir`.
/// `None` when the file holds no 2.x key, which leaves every file alone.
///
/// `save_secrets` stores the web-search keys 2.x kept in plaintext, given
/// the credential store the file selects, and returns the credentials
/// whose stored key differs and was kept. Its failure does not stop the
/// upgrade: the keys leave `config.toml` either way, since 3.0 refuses them
/// there.
pub(crate) fn upgrade_2x(
    config_path: &Path,
    rho_dir: &Path,
    save_secrets: impl FnOnce(
        Option<CredentialStoreBackend>,
        &[(WebSearchCredential, String)],
    ) -> anyhow::Result<Vec<WebSearchCredential>>,
) -> anyhow::Result<Option<Upgraded>> {
    let Some(original) = read(config_path)? else {
        return Ok(None);
    };
    // Loading reports a file that does not parse.
    let Ok(doc) = original.parse::<DocumentMut>() else {
        return Ok(None);
    };
    let migrated = migrate_2x(doc);
    if migrated.report.is_empty() {
        return Ok(None);
    }

    let backup = back_up(config_path, &original)?;
    let mut already_set = Vec::new();
    for (file, path, moved) in [
        (
            MigratedFile::ClientTui,
            rho_dir.join("clients").join("tui.toml"),
            migrated.client_tui,
        ),
        (
            MigratedFile::TuiLocal,
            rho_dir.join("tui.toml"),
            migrated.tui_local,
        ),
    ] {
        let existing = read(&path)?;
        let mut merged = match &existing {
            Some(text) => ConfigDocument::parse(text)
                .with_context(|| format!("failed to parse {}", path.display()))?,
            None => ConfigDocument::default(),
        };
        let moved = ConfigDocument::from(moved);
        for key in moved.leaf_paths().iter().map(|leaf| leaf.join(".")) {
            let new = moved.get(&key);
            match merged.get(&key) {
                None => merged.set_item(&key, new.cloned()),
                // An interrupted upgrade already wrote this value.
                Some(kept) if same_value(kept, new) => {}
                Some(_) => already_set.push(name_in_2x(&migrated.report, file, &key)),
            }
        }
        let text = merged.text();
        if text == existing.clone().unwrap_or_default() {
            continue;
        }
        if let Some(existing) = &existing {
            back_up(&path, existing)?;
        }
        write(&path, &text)?;
    }

    let agent = ConfigDocument::from(migrated.agent);
    let mut secrets_error = None;
    if !migrated.secrets.is_empty() {
        let backend = agent
            .get("behavior.credential_store")
            .and_then(Item::as_str)
            .and_then(|backend| CredentialStoreBackend::parse(backend).ok());
        match save_secrets(backend, &migrated.secrets) {
            Ok(kept) => {
                // `migrate_2x` reports one `Secret` per entry of `secrets`,
                // in the same order.
                let secret_keys = migrated.report.iter().filter_map(|change| match change {
                    Change::Secret { key } => Some(key),
                    Change::Moved { .. } | Change::Rewritten { .. } | Change::Dropped { .. } => {
                        None
                    }
                });
                already_set.extend(
                    migrated
                        .secrets
                        .iter()
                        .zip(secret_keys)
                        .filter(|((credential, _), _)| kept.contains(credential))
                        .map(|(_, key)| key.clone()),
                );
            }
            Err(error) => secrets_error = Some(format!("{error:#}")),
        }
    }
    let upgraded = Upgraded {
        backup,
        report: migrated.report,
        already_set,
        secrets_error,
    };
    write(config_path, &agent.text())?;
    // After `config.toml`, so the note tells only of an upgrade that was
    // done. A note that cannot be written costs only the TUI's copy of the
    // notice.
    if let Err(error) = super::upgrade_2x_note::leave(rho_dir, config_path, &upgraded) {
        tracing::warn!("could not leave the config upgrade note: {error:#}");
    }
    Ok(Some(upgraded))
}

/// Whether a line of an upgrade's report needs the user to act.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    Note,
    Warning,
}

impl Upgraded {
    /// The lines startup prints: where the original is, what moved where,
    /// and one warning for everything that was not carried over.
    pub(crate) fn notice(&self, config_path: &Path) -> Vec<String> {
        let header = format!(
            "note: updated {} from the rho 2.x layout; the original is at {}",
            config_path.display(),
            self.backup.display()
        );
        std::iter::once(header)
            .chain(self.changes().into_iter().map(|(severity, line)| {
                let prefix = match severity {
                    Severity::Note => "note",
                    Severity::Warning => "warning",
                };
                format!("{prefix}: {line}")
            }))
            .collect()
    }

    /// What the upgrade changed and what it left out, one line each. Both
    /// the startup notice and the TUI's note list these.
    pub(crate) fn changes(&self) -> Vec<(Severity, String)> {
        let mut lines = Vec::new();
        for (file, name) in [
            (MigratedFile::Agent, "their 3.0 keys in config.toml"),
            (MigratedFile::ClientTui, "clients/tui.toml"),
            (MigratedFile::TuiLocal, "tui.toml"),
        ] {
            let moved = self
                .report
                .iter()
                .filter_map(|change| match change {
                    Change::Moved { from, file: to, .. } if *to == file => Some(from.as_str()),
                    Change::Moved { .. }
                    | Change::Rewritten { .. }
                    | Change::Dropped { .. }
                    | Change::Secret { .. } => None,
                })
                .collect::<Vec<_>>();
            if !moved.is_empty() {
                lines.push((
                    Severity::Note,
                    format!("moved to {name}: {}", moved.join(", ")),
                ));
            }
        }
        let mut ignored = Vec::new();
        let mut stored = Vec::new();
        for change in &self.report {
            match change {
                Change::Moved { .. } => {}
                Change::Rewritten { key, from, to } => {
                    lines.push((Severity::Note, format!("`{key}`: {from} is now {to}")));
                }
                Change::Dropped { key } => ignored.push(key.as_str()),
                Change::Secret { key } if self.already_set.contains(key) => {}
                Change::Secret { key } => stored.push(key.as_str()),
            }
        }
        ignored.extend(self.already_set.iter().map(String::as_str));
        if !ignored.is_empty() {
            lines.push((
                Severity::Warning,
                format!(
                    "ignored 2.x keys that 3.0 does not read, or that 3.0 already sets to another value: {}",
                    ignored.join(", ")
                ),
            ));
        }
        match (&self.secrets_error, stored.is_empty()) {
            (_, true) => {}
            (None, false) => lines.push((
                Severity::Note,
                format!("moved to the credential store: {}", stored.join(", ")),
            )),
            (Some(error), false) => lines.push((
                Severity::Warning,
                format!(
                    "could not move {} to the credential store ({error}); set them again from the copy in {}",
                    stored.join(", "),
                    self.backup.display()
                ),
            )),
        }
        lines
    }
}

fn same_value(kept: &Item, new: Option<&Item>) -> bool {
    match (kept.as_value(), new.and_then(Item::as_value)) {
        (Some(kept), Some(new)) => shown(kept) == shown(new),
        _ => false,
    }
}

/// The 2.x key whose value moved to `key` in `file`. `[keybindings]` moves
/// as one table, so a chord is named under the table's 2.x key.
fn name_in_2x(report: &[Change], file: MigratedFile, key: &str) -> String {
    report
        .iter()
        .find_map(|change| match change {
            Change::Moved {
                from,
                file: to_file,
                to,
            } if *to_file == file => {
                let rest = key.strip_prefix(to.as_str())?;
                (rest.is_empty() || rest.starts_with('.')).then(|| format!("{from}{rest}"))
            }
            Change::Moved { .. }
            | Change::Rewritten { .. }
            | Change::Dropped { .. }
            | Change::Secret { .. } => None,
        })
        .unwrap_or_else(|| key.to_owned())
}

/// The text of `path`, or `None` when no file is there.
fn read(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn write(path: &Path, text: &str) -> anyhow::Result<()> {
    write_bytes_atomically(path, text.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Saves `contents` beside `path` as `<name>.2x.bak` and returns where. A
/// backup already holding `contents`, as an interrupted upgrade leaves, is
/// reused; one holding anything else is kept, and a numbered name is used.
fn back_up(path: &Path, contents: &str) -> anyhow::Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no file name", path.display()))?;
    let mut backup = path.with_file_name(format!("{name}.2x.bak"));
    let mut number = 0;
    while let Some(kept) = read(&backup)? {
        if kept == contents {
            return Ok(backup);
        }
        number += 1;
        backup = path.with_file_name(format!("{name}.2x.bak.{number}"));
    }
    write(&backup, contents)?;
    Ok(backup)
}

#[cfg(test)]
#[path = "config_upgrade_2x_tests.rs"]
mod tests;
