//! Which stale revisions a write may still be made against: those its
//! writer's own unbroken run of changes passed through. A client that
//! sends a write before the daemon's `rho/config/changed` for its last one
//! reached it stamps the revision it last heard of; every change since is
//! its own, so the write is not a conflict. Only the daemon sees every
//! change, so only it can tell.

use std::collections::HashSet;

use rho_protocol::{ConfigRevision, ConfigWriter, ErrorCode, RpcError};

/// One namespace's latest run of changes by a single writer.
#[derive(Default)]
pub(super) struct WriterRun(Option<Run>);

struct Run {
    writer: ConfigWriter,
    /// The revision the run's last change produced. The run holds only
    /// while the namespace is still there: any other change, a hand edit
    /// included, moves the namespace off it.
    tip: ConfigRevision,
    /// Every revision the run passed through, the one it started from
    /// included. Grows by at most one per change, and only with distinct
    /// states one writer visits with no one else changing the namespace.
    passed: HashSet<ConfigRevision>,
}

/// A write [`WriterRun::admit`] let through, to hand back to
/// [`WriterRun::landed`] once it is saved.
#[must_use = "a saved write must be recorded with `WriterRun::landed`"]
pub(super) struct Admitted {
    writer: Option<ConfigWriter>,
    /// The namespace's revision the write lands on.
    before: ConfigRevision,
}

impl WriterRun {
    /// Lets through a write `writer` made against `sent`, landing on
    /// `current`; `ConfigConflict` when someone else changed the namespace
    /// since `sent`.
    pub(super) fn admit(
        &self,
        current: ConfigRevision,
        sent: &ConfigRevision,
        writer: Option<ConfigWriter>,
    ) -> Result<Admitted, RpcError> {
        let own = self.0.as_ref().is_some_and(|run| {
            writer.as_ref() == Some(&run.writer) && run.tip == current && run.passed.contains(sent)
        });
        if current == *sent || own {
            Ok(Admitted {
                writer,
                before: current,
            })
        } else {
            Err(RpcError::new(
                ErrorCode::ConfigConflict,
                "the settings changed since that revision; read them again",
            ))
        }
    }

    /// Records that `write` saved, leaving the namespace at `after`. A
    /// write that changed nothing leaves any run as it was.
    pub(super) fn landed(&mut self, write: Admitted, after: ConfigRevision) {
        let Admitted { writer, before } = write;
        if before == after {
            return;
        }
        let Some(writer) = writer else {
            self.0 = None;
            return;
        };
        match &mut self.0 {
            Some(run) if run.writer == writer && run.tip == before => {
                run.passed.insert(after.clone());
                run.tip = after;
            }
            _ => {
                self.0 = Some(Run {
                    writer,
                    passed: HashSet::from([before, after.clone()]),
                    tip: after,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "writer_run_tests.rs"]
mod tests;
