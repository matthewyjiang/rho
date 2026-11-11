//! The cards of the calls a provider is still streaming, which
//! [`LiveEvents`] sends as their arguments grow.

use rho_protocol::{display::Presentation, SessionEvent as WireEvent, ToolCallUpdate, ToolStatus};
use rho_sdk::tool::ToolMetadata;

use super::{named_call, status_update, CallState, LiveEvents};

/// The card of a call the provider is still streaming, and how much of it
/// clients have.
///
/// A card can hold all the arguments so far, a file's content for one, so
/// sending each would cost the square of their size. One goes out when the
/// arguments grew by a quarter since the last: the cards sent then sum to
/// about five times the arguments (1 / (1 - 1/1.25)), however long they get.
/// A host that sees the stream pause sends the one held back
/// ([`LiveEvents::held_previews`]).
#[derive(Default)]
pub(super) struct StreamedPreview {
    /// The call's provider id and name, once the provider said them.
    call: Option<(String, String)>,
    /// The newest card clients were not sent; it waits for the call's id
    /// and name, and for the arguments to grow enough or the stream to
    /// pause.
    unsent: Option<rho_protocol::display::ToolCard>,
    /// Bytes of arguments received.
    received: usize,
    /// `received` when a card was last sent; 0 before the first.
    sent_at: usize,
}

impl LiveEvents {
    /// The card of a call the provider is still streaming: more of the
    /// arguments of the call at `index` of its output, which names its id
    /// and tool once it knows them. Nothing until the call has an id, a
    /// name, and arguments worth showing; then the call opens `pending`
    /// with the card the TUI draws for it, and each change of that card
    /// follows.
    pub(super) fn streamed_call(
        &mut self,
        index: usize,
        id: Option<&str>,
        name: Option<&str>,
        arguments_delta: &str,
    ) -> Vec<WireEvent> {
        let presented = self
            .presenter
            .preview(index, name.map(str::to_owned), arguments_delta);
        let preview = self.previews.entry(index).or_default();
        preview.received += arguments_delta.len();
        if let Some(presented) = presented {
            preview.unsent = Some(presented.card);
        }
        // The SDK repeats a call's id and name on each delta once known.
        if let (Some(provider), Some(name)) = (id.filter(|id| !id.is_empty()), name) {
            preview.call = Some((provider.to_owned(), name.to_owned()));
        }
        if preview.sent_at > 0 && preview.received < preview.sent_at + preview.sent_at / 4 {
            return Vec::new();
        }
        self.send_preview(index)
    }

    /// The cards of streamed calls held back for their arguments to grow
    /// ([`StreamedPreview`]). A host calls this when the stream pauses, so
    /// what the model wrote last shows without waiting for more. A card
    /// ends the open message, as any event but a chunk does.
    pub(crate) fn held_previews(&mut self) -> Vec<WireEvent> {
        let held: Vec<usize> = self
            .previews
            .iter()
            .filter(|(_, preview)| preview.unsent.is_some())
            .map(|(index, _)| *index)
            .collect();
        let cards: Vec<_> = held
            .into_iter()
            .flat_map(|index| self.send_preview(index))
            .collect();
        if !cards.is_empty() {
            // Called outside [`Self::map`] too.
            self.message = None;
        }
        cards
    }

    /// Whether [`Self::held_previews`] has a card to send.
    pub(crate) fn holds_previews(&self) -> bool {
        self.previews
            .values()
            .any(|preview| preview.unsent.is_some() && preview.call.is_some())
    }

    /// Sends the unsent card of the streamed call at `index`, once the call
    /// has an id and a name: it opens the call `pending`, or follows it.
    fn send_preview(&mut self, index: usize) -> Vec<WireEvent> {
        let Some(preview) = self.previews.get_mut(&index) else {
            return Vec::new();
        };
        let Some((provider, name)) = preview.call.clone() else {
            return Vec::new();
        };
        let Some(card) = preview.unsent.take() else {
            return Vec::new();
        };
        let first = preview.sent_at == 0;
        preview.sent_at = preview.received.max(1);
        let display = Some(Presentation::Card(card));
        match self
            .announced
            .get(&provider)
            .map(|call| (call.state, &call.id))
        {
            Some((CallState::Pending, id)) => {
                return vec![WireEvent::tool_call(ToolCallUpdate {
                    display,
                    ..status_update(id.clone(), /*status*/ None)
                })];
            }
            // It started; its own updates draw it from here.
            Some((CallState::Open, _)) => return Vec::new(),
            // This step's call, which a card it held back outlived.
            Some((CallState::Finished, _)) if !first => return Vec::new(),
            // New, or a later step reusing a finished call's id.
            Some((CallState::Finished, _)) | None => {}
        }
        let (wire_id, superseded) = self.announce(&provider, CallState::Pending);
        let opened = ToolCallUpdate {
            status: Some(ToolStatus::Pending),
            display,
            ..named_call(
                wire_id,
                &name,
                /*arguments*/ None,
                &ToolMetadata::default(),
                &self.workspace,
                &self.mcp_tools,
            )
        };
        superseded
            .into_iter()
            .chain([WireEvent::tool_call(opened)])
            .collect()
    }
}
