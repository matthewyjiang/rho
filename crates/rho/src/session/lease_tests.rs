use std::fs::{File, OpenOptions};

use pretty_assertions::assert_eq;
use rho_providers::model::Message;
use tempfile::TempDir;

use super::{acquire, lock_path, InUseBy, LeasePurpose, SessionInUse};
use crate::session::{DeleteOptions, Session};

/// A saved, closed session and the roots it is stored under.
struct Saved {
    root: TempDir,
    subagents: TempDir,
    cwd: TempDir,
    id: String,
}

impl Saved {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
        session
            .append_message(&Message::assistant_text("saved"))
            .unwrap();
        Self {
            id: session.id().to_owned(),
            root,
            subagents: tempfile::tempdir().unwrap(),
            cwd,
        }
    }

    fn open(&self) -> anyhow::Result<Session> {
        Session::open_by_id_in_root(self.root.path(), self.cwd.path(), &self.id)
            .map(|(session, _)| session)
    }

    fn delete(&self) -> anyhow::Result<()> {
        Session::delete_by_id_in_roots(
            self.root.path(),
            self.subagents.path(),
            self.cwd.path(),
            &self.id,
            DeleteOptions::default(),
        )
        .map(drop)
    }

    /// Locks the session the way another Rho process does: on a file handle
    /// of its own, which this process's leases do not count. `None` when
    /// something holds the session.
    fn lock_elsewhere(&self) -> Option<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path(self.root.path(), self.cwd.path(), &self.id))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&file).ok().map(|()| file)
    }
}

/// Who holds the session when this process opens or deletes it.
#[derive(Clone, Copy, Debug)]
enum Holder {
    Nobody,
    ThisProcessOpen,
    ThisProcessDeleting,
    /// Another process that has the session open, or is deleting it.
    OtherProcess,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Open,
    Delete,
}

/// `Ok(())` when the action went through, else what [`SessionInUse`] says
/// holds the session. Any other error fails the test.
fn outcome<T>(result: anyhow::Result<T>) -> Result<(), InUseBy> {
    match result {
        Ok(_) => Ok(()),
        Err(error) => Err(error
            .downcast_ref::<SessionInUse>()
            .unwrap_or_else(|| panic!("not a SessionInUse refusal: {error:#}"))
            .by()),
    }
}

// Covers: one process writes a session file at a time. Another process's
// open or delete of a session this one holds is refused, while opens within
// one process share it (an ACP session/load replacing a live session); a
// closed session opens and deletes again.
// Owner: session lease
#[test]
fn a_session_has_one_writer_process() {
    let cases = [
        (Holder::Nobody, Action::Open, Ok(())),
        (Holder::ThisProcessOpen, Action::Open, Ok(())),
        (
            Holder::ThisProcessDeleting,
            Action::Open,
            Err(InUseBy::DeletingHere),
        ),
        (
            Holder::OtherProcess,
            Action::Open,
            Err(InUseBy::OtherProcess),
        ),
        (Holder::Nobody, Action::Delete, Ok(())),
        (
            Holder::ThisProcessOpen,
            Action::Delete,
            Err(InUseBy::OpenHere),
        ),
        (
            Holder::OtherProcess,
            Action::Delete,
            Err(InUseBy::OtherProcess),
        ),
    ];
    for (holder, action, expected) in cases {
        let saved = Saved::new();
        let _hold: Box<dyn std::any::Any> = match holder {
            Holder::Nobody => Box::new(()),
            Holder::ThisProcessOpen => Box::new(saved.open().unwrap()),
            Holder::ThisProcessDeleting => Box::new(
                acquire(
                    saved.root.path(),
                    saved.cwd.path(),
                    &saved.id,
                    LeasePurpose::Delete,
                )
                .unwrap(),
            ),
            Holder::OtherProcess => Box::new(saved.lock_elsewhere().unwrap()),
        };
        let actual = match action {
            Action::Open => outcome(saved.open()),
            Action::Delete => outcome(saved.delete()),
        };
        assert_eq!(actual, expected, "{holder:?} {action:?}");
    }
}

// Covers: every open of a session in this process shares one lock, which
// another process can take only once the last of them closes, as when an ACP
// session/load reopens a live session and the replaced runtime closes after.
// Owner: session lease
#[test]
fn another_process_is_refused_until_the_last_open_here_closes() {
    let saved = Saved::new();
    let first = saved.open().unwrap();
    let clone = first.clone();
    let replacement = saved.open().unwrap();

    let mut lockable_elsewhere = Vec::new();
    drop(first);
    lockable_elsewhere.push(saved.lock_elsewhere().is_some());
    drop(clone);
    lockable_elsewhere.push(saved.lock_elsewhere().is_some());
    drop(replacement);
    lockable_elsewhere.push(saved.lock_elsewhere().is_some());

    assert_eq!(lockable_elsewhere, [false, false, true]);
}
