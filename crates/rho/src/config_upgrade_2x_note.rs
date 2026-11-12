//! The note the TUI shows once after a 2.x `config.toml` was upgraded.
//!
//! Startup prints the upgrade notice to stderr, which the TUI's screen covers
//! a moment later. So [`super::upgrade_2x`], in whichever rho process runs it
//! (the TUI, `rho acp`, `rho run`, or the daemon), also leaves this note in
//! the rho home, and the next TUI to start takes it and shows it in its
//! transcript. Nothing else reads the note, so it reaches no session or
//! model.

use std::path::Path;

use anyhow::Context;

use super::{
    migrate_2x::{Change, MigratedFile},
    upgrade_2x::Upgraded,
};
use crate::config_writer::write_bytes_atomically;

/// The note's file in the rho home, until a TUI takes it.
const NOTE_FILE: &str = "config-2x-upgrade-note.txt";

const UPGRADE_GUIDE: &str = "https://matthewyjiang.github.io/rho/upgrading-to-3#configuration";

/// Leaves the note on `upgraded` for the next TUI, replacing one no TUI took.
pub(super) fn leave(rho_dir: &Path, config_path: &Path, upgraded: &Upgraded) -> anyhow::Result<()> {
    let path = rho_dir.join(NOTE_FILE);
    write_bytes_atomically(&path, text(config_path, upgraded).as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Takes the note an upgrade left, so no later TUI shows it. `None` when
/// there is none, or another TUI took it first.
pub(crate) fn take_upgrade_note(rho_dir: &Path) -> Option<String> {
    let path = rho_dir.join(NOTE_FILE);
    let note = match std::fs::read_to_string(&path) {
        Ok(note) => note,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not read the config upgrade note");
            return None;
        }
    };
    // Of two TUIs starting together, the one whose removal succeeds shows it.
    // A note that cannot be removed is not shown, rather than shown each start.
    match std::fs::remove_file(&path) {
        Ok(()) => Some(note),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not remove the config upgrade note");
            None
        }
    }
}

/// What the upgrade did, in words for someone who has not read the 2.x keys
/// in a while: what moved where, what was left out, where the original is.
fn text(config_path: &Path, upgraded: &Upgraded) -> String {
    let mut renamed = false;
    let mut tui_local = Vec::new();
    let mut client_tui = Vec::new();
    let mut stored = false;
    let mut left_out = Vec::new();
    for change in &upgraded.report {
        match change {
            Change::Moved {
                file: MigratedFile::Agent,
                ..
            }
            | Change::Rewritten { .. } => renamed = true,
            Change::Moved {
                from,
                file: MigratedFile::TuiLocal,
                ..
            } => tui_local.push(from.as_str()),
            Change::Moved {
                from,
                file: MigratedFile::ClientTui,
                ..
            } => client_tui.push(from.as_str()),
            Change::Dropped { key } => left_out.push(key.as_str()),
            Change::Secret { key } if upgraded.already_set.contains(key) => {}
            Change::Secret { .. } => stored = true,
        }
    }
    left_out.extend(upgraded.already_set.iter().map(String::as_str));

    let mut lines = vec![format!(
        "rho 3 updated your config from the 2.x layout: {}",
        crate::paths::display(config_path)
    )];
    if renamed {
        lines.push("- settings rho 3 renamed now use their new names in config.toml".to_owned());
    }
    for (file, keys) in [("tui.toml", tui_local), ("clients/tui.toml", client_tui)] {
        if !keys.is_empty() {
            lines.push(format!("- moved to {file}: {}", keys.join(", ")));
        }
    }
    match (&upgraded.secrets_error, stored) {
        (_, false) => {}
        (None, true) => {
            lines.push("- web-search API keys moved to your credential store".to_owned());
        }
        (Some(error), true) => lines.push(format!(
            "- could not move web-search API keys to your credential store ({error}); set them again from the original"
        )),
    }
    if !left_out.is_empty() {
        lines.push(format!(
            "- left out, because rho 3 does not read them or already sets them: {}",
            left_out.join(", ")
        ));
    }
    lines.push(format!(
        "the original is saved at {}",
        crate::paths::display(&upgraded.backup)
    ));
    lines.push(format!("what changed in rho 3: {UPGRADE_GUIDE}"));
    lines.join("\n")
}
