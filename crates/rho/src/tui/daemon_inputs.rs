//! A daemon session's open input requests as the TUI follows them (#1667):
//! which are open, which the TUI answered, and which to show.
//!
//! The daemon asks every attached client, and the first answer wins. The
//! attach response lists the requests open when the attachment began, and
//! its replay may repeat some, or open and close others, so requests are
//! keyed by id. While the replay runs, input events only change what is
//! open: once it ends, each request still open shows, oldest first, and one
//! the replay opened and closed never shows.
//!
//! A request the TUI answered closes silently, as its prompt already did.
//! One another client answered, the turn withdrew, or the daemon answered at
//! its timeout closes with a notice saying so.

use std::collections::HashSet;

use rho_protocol::{Ask, InputRequest, InputRequestId, Resolution};

use super::daemon_events::DaemonView;

/// Why the daemon took no answer the TUI sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AnswerRefusal {
    /// The request closed first: another client answered it, it timed out,
    /// or the turn withdrew it. The error row says so, and its resolution,
    /// if still to come, adds nothing.
    NotOpen,
    /// The daemon refused it, or could not be asked, for this reason; the
    /// request is still open.
    Failed(String),
}

/// The open input requests of one attachment.
#[derive(Default)]
pub(super) struct OpenInputs {
    /// Open requests, in the order they opened.
    open: Vec<InputRequest>,
    /// Requests the TUI answered whose resolution is still to come.
    answered: HashSet<InputRequestId>,
    /// Whether the attachment's replay is still arriving.
    replaying: bool,
}

impl OpenInputs {
    /// Starts an attachment whose response lists `inputs` as open; its
    /// replay follows, until [`Self::end_replay`].
    pub(super) fn attached(&mut self, inputs: Vec<InputRequest>) {
        *self = Self {
            open: inputs,
            answered: HashSet::new(),
            replaying: true,
        };
    }

    /// Whether the attachment's replay is still arriving.
    pub(super) fn replaying(&self) -> bool {
        self.replaying
    }

    /// The replay is over: every request still open shows, oldest first.
    pub(super) fn end_replay(&mut self) -> Vec<DaemonView> {
        self.replaying = false;
        self.open
            .iter()
            .filter(|request| !self.answered.contains(&request.id))
            .map(|request| DaemonView::InputOpened(Box::new(request.clone())))
            .collect()
    }

    /// Request `request` opened; it shows unless it is open already, as a
    /// request the attach listed and its replay repeats is.
    pub(super) fn requested(&mut self, request: InputRequest) -> Option<DaemonView> {
        if self.open.iter().any(|open| open.id == request.id) {
            return None;
        }
        self.open.push(request.clone());
        (!self.replaying).then(|| DaemonView::InputOpened(Box::new(request)))
    }

    /// Request `id` closed as `resolution` says.
    pub(super) fn resolved(
        &mut self,
        id: InputRequestId,
        resolution: Resolution,
    ) -> Option<DaemonView> {
        let index = self.open.iter().position(|open| open.id == id)?;
        let request = self.open.remove(index);
        if self.answered.remove(&id) || self.replaying {
            return None;
        }
        Some(DaemonView::InputClosed {
            id,
            notice: closed_notice(&request, resolution),
        })
    }

    /// Request `id` no longer times out: a client held it.
    pub(super) fn held(&mut self, id: InputRequestId) -> Option<DaemonView> {
        let request = self.open.iter_mut().find(|open| open.id == id)?;
        request.timeout = None;
        (!self.replaying).then_some(DaemonView::InputHeld(id))
    }

    /// The TUI sent an answer to request `id`: its resolution closes it
    /// silently.
    pub(super) fn answering(&mut self, id: InputRequestId) {
        self.answered.insert(id);
    }

    /// The daemon took no answer to request `id`, as `refusal` says: an
    /// error row, then, while the request is still open, its prompt again.
    pub(super) fn refused(
        &mut self,
        id: InputRequestId,
        refusal: AnswerRefusal,
    ) -> Vec<DaemonView> {
        self.answered.remove(&id);
        match refusal {
            AnswerRefusal::NotOpen => {
                // Its resolution, if still to come, finds it gone.
                self.open.retain(|open| open.id != id);
                vec![DaemonView::Error(
                    "could not answer the request: it is no longer open".into(),
                )]
            }
            AnswerRefusal::Failed(detail) => {
                let mut views = vec![DaemonView::Error(format!(
                    "could not answer the request: {detail}"
                ))];
                views.extend(
                    self.open
                        .iter()
                        .find(|open| open.id == id)
                        .map(|request| DaemonView::InputOpened(Box::new(request.clone()))),
                );
                views
            }
        }
    }
}

/// How a request the TUI did not answer closed, as a notice.
fn closed_notice(request: &InputRequest, resolution: Resolution) -> String {
    let asked = match &request.ask {
        Ask::Approval(_) => "the approval",
        Ask::PlanApproval(_) => "the plan approval",
        Ask::Question(_) => "the questions",
        // `Unknown`, or an ask a newer daemon sends.
        _ => "the request",
    };
    match resolution {
        Resolution::Answered => format!("another client answered {asked}"),
        Resolution::Withdrawn => format!("the turn withdrew {asked}"),
        Resolution::TimedOut => match &request.timeout {
            Some(timeout) => format!("timeout fallback: {}", timeout.reason),
            None => format!("the daemon answered {asked} at its timeout"),
        },
        // `Unknown`, or a resolution a newer daemon sends.
        _ => format!("{asked} closed"),
    }
}
