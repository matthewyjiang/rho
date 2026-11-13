//! The note the TUI shows once after a 2.x `config.toml` was upgraded.
//!
//! Startup prints the upgrade notice to stderr, which the TUI's screen covers
//! a moment later. So [`super::upgrade_2x`], in whichever rho process runs it
//! (the TUI, `rho acp`, `rho run`, or the daemon), also leaves this note in
//! the rho home, and the next TUI to start takes it and shows it in its
//! transcript. Nothing else reads the note, so it reaches no session or
//! model.
//!
//! The TUI removes the note before its first frame. A TUI that then quits
//! before the note is drawn, or whose transcript is reset by a session
//! switch or a daemon replay, does not show it again: once beats every start.

use std::path::Path;

use anyhow::Context;

use super::upgrade_2x::{Severity, Upgraded};
use crate::config_writer::write_bytes_atomically;

/// The note's file in the rho home, until a TUI takes it.
const NOTE_FILE: &str = "config-2x-upgrade-note.txt";

const UPGRADE_GUIDE: &str = "https://matthewyjiang.github.io/rho/upgrading-to-3#configuration";

/// Leaves the note on `upgraded` for the next TUI, after any note no TUI
/// took yet: an earlier upgrade's note names the backup of the full 2.x file.
pub(super) fn leave(rho_dir: &Path, config_path: &Path, upgraded: &Upgraded) -> anyhow::Result<()> {
    let path = rho_dir.join(NOTE_FILE);
    let mut note = match std::fs::read_to_string(&path) {
        Ok(earlier) => earlier + "\n\n",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    };
    note.push_str(&text(config_path, upgraded));
    write_bytes_atomically(&path, note.as_bytes())
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

/// What the upgrade did: the startup notice's lines, as a list under a
/// header with warnings marked, then where the original is and where to read more.
fn text(config_path: &Path, upgraded: &Upgraded) -> String {
    let header = format!(
        "rho 3 updated your config from the 2.x layout: {}",
        crate::paths::display(config_path)
    );
    let changes = upgraded
        .changes()
        .into_iter()
        .map(|(severity, line)| match severity {
            Severity::Note => format!("- {line}"),
            Severity::Warning => format!("- warning: {line}"),
        });
    let footer = [
        format!(
            "the original is saved at {}",
            crate::paths::display(&upgraded.backup)
        ),
        format!("what changed in rho 3: {UPGRADE_GUIDE}"),
    ];
    std::iter::once(header)
        .chain(changes)
        .chain(footer)
        .collect::<Vec<_>>()
        .join("\n")
}
