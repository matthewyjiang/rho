//! Rebuild a client's view of a session from its saved display rows,
//! without running tools.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
};

use rho_protocol::{
    display::Presentation, MessageId, PlanEntry, SessionEvent as WireEvent, ToolCallId,
    ToolCallUpdate, ToolStatus,
};
use rho_sdk::{
    model::{self, Message, SemanticMessage, ToolResult},
    tool::ToolMetadata,
};

use super::{
    cards, encoded_len, opened_call, plan_entries, result_content, status_update, user_content,
    CallIds, UNNAMED_TOOL_TITLE,
};
use crate::{
    app::{interactive_presenter::InteractiveToolPresenter, session_runtime::SessionTools},
    display_transcript::DisplayTranscript,
    session::tree::{DisplayRowOrigin, ProjectedMessage},
    tools::mcp::exported_name::McpToolNames,
};

/// Display rows a session saved together, such as one turn's, kept to
/// replay to a client that attaches with a reset ([`SavedTranscript`]).
///
/// The rows share their messages with the session's history. The events
/// they replay as would not: each tool output appears in them twice, as the
/// call's content and in its card. So the turn keeps the rows, and with
/// them what replay could not derive from the rows again, as it was when
/// they saved: each call's wire id and start metadata, the session's MCP
/// names and its plan. A replay renders the same events from them every
/// time, a row at a time as its client takes them.
///
/// Each saved message replays whole: one event per message, never chunks.
/// Message ids derive from the row a message was saved in, and tool call
/// ids from the provider's saved ids, numbered after the rows saved before.
/// A call the history left without a result, as a killed process does,
/// closes `failed`, since history is not running work. The turn's replay
/// ends with the session's plan as it stood, when it had one.
///
/// Calls carry the cards Rho's TUI draws for them
/// (`ToolCallUpdate::display`): a call opens with the card live draws at its
/// start, from the saved call alone (`InteractiveToolPresenter::for_saved_calls`),
/// and a result brings the card the TUI restores from history. The call an
/// aborted response was still writing gets none: the wire replays no
/// partial calls.
#[derive(Debug)]
pub(crate) struct SavedTurn {
    rows: Vec<ProjectedMessage>,
    /// Session working directory; titles show paths relative to it.
    workspace: PathBuf,
    /// The tools' MCP names: they identify MCP calls, whose capped names
    /// cannot be decoded.
    mcp_tools: McpToolNames,
    /// Each call the rows open, in the order replay opens them.
    calls: Vec<OpenedCall>,
    plan: Vec<PlanEntry>,
    /// Where in the turn's events the ones JSON cannot hold are, such as one
    /// with a non-UTF-8 path, which no client is sent: its connection would
    /// fail on it. Ascending.
    unencodable: Vec<usize>,
    /// How many events a client is sent.
    sent: usize,
}

/// A call replay opened: its wire id, and what its tool reported at start.
/// Saved calls keep no metadata, so each tool reported what it would at
/// start, as it does live; a result whose call history lost has none.
#[derive(Clone, Debug)]
struct OpenedCall {
    id: ToolCallId,
    start: ToolMetadata,
}

impl SavedTurn {
    /// `rows`, a saved session's display rows (`SavedHistory::display_rows`),
    /// as `tools` describe them now. Numbers their calls after those `ids`
    /// has counted.
    pub(super) fn capture(
        rows: &[ProjectedMessage],
        workspace: &Path,
        tools: SessionTools<'_>,
        ids: &mut CallIds,
    ) -> Self {
        let mcp_tools = tools.mcp_report().tool_names();
        let plan = plan_entries(&tools.todo_state());
        let mut calls = Capture {
            tools,
            ids,
            opened: Vec::new(),
        };
        // Rendering once, a row at a time, records the calls and finds the
        // events no client can be sent, without holding the events.
        let mut replay = Replay::new(workspace, &mcp_tools);
        let mut unencodable = Vec::new();
        let mut rendered = 0;
        let mut check = |events: Vec<WireEvent>| {
            for event in events {
                if encoded_len(&event).is_none() {
                    unencodable.push(rendered);
                }
                rendered += 1;
            }
        };
        for row in rows {
            replay.row(row, &mut calls);
            check(replay.take_events());
        }
        replay.finish(&plan);
        check(replay.take_events());
        Self {
            rows: rows.to_vec(),
            workspace: workspace.to_owned(),
            mcp_tools,
            calls: calls.opened,
            plan,
            sent: rendered - unencodable.len(),
            unencodable,
        }
    }
}

/// The events a client is sent to replay one [`SavedTurn`], rendered a row
/// at a time as it takes them.
struct TurnEvents {
    turn: Arc<SavedTurn>,
    replay: Replay,
    /// The next row to render.
    row: usize,
    /// The next of the turn's calls to open.
    call: usize,
    finished: bool,
    /// Rendered and not taken yet.
    pending: VecDeque<WireEvent>,
    /// How many events were rendered before `pending`'s first.
    rendered: usize,
}

impl TurnEvents {
    fn new(turn: Arc<SavedTurn>) -> Self {
        Self {
            replay: Replay::new(&turn.workspace, &turn.mcp_tools),
            turn,
            row: 0,
            call: 0,
            finished: false,
            pending: VecDeque::new(),
            rendered: 0,
        }
    }
}

impl Iterator for TurnEvents {
    type Item = WireEvent;

    fn next(&mut self) -> Option<WireEvent> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                let at = self.rendered;
                self.rendered += 1;
                if self.turn.unencodable.binary_search(&at).is_err() {
                    return Some(event);
                }
                continue;
            }
            let mut calls = Recorded {
                calls: &self.turn.calls,
                next: &mut self.call,
            };
            match self.turn.rows.get(self.row) {
                Some(row) => {
                    self.replay.row(row, &mut calls);
                    self.row += 1;
                }
                None if !self.finished => {
                    self.replay.finish(&self.turn.plan);
                    self.finished = true;
                }
                None => {
                    // Rendering from the rows alone, the replay opens the
                    // calls capture recorded and sends what it counted.
                    debug_assert_eq!(self.call, self.turn.calls.len());
                    debug_assert_eq!(self.rendered, self.turn.sent + self.turn.unencodable.len());
                    return None;
                }
            }
            self.pending.extend(self.replay.take_events());
        }
    }
}

/// The turns a session saved, in order, as a client that resets replays
/// them.
#[derive(Debug, Default)]
pub(crate) struct SavedTranscript {
    /// Shared with the replays under way, which render them as their
    /// clients take the events.
    turns: Vec<Arc<SavedTurn>>,
    /// How many events a replay sends.
    sent: usize,
}

impl SavedTranscript {
    pub(crate) fn push(&mut self, turn: SavedTurn) {
        self.sent += turn.sent;
        self.turns.push(Arc::new(turn));
    }

    /// How many events [`Self::events`] yields.
    pub(crate) fn len(&self) -> usize {
        self.sent
    }

    /// The wire events that replay every turn, in order, the same each
    /// time. They render a row at a time as the replay is taken, so a reset
    /// never holds a long session's events at once.
    pub(crate) fn events(&self) -> impl Iterator<Item = WireEvent> + Send + Sync + 'static {
        self.turns.clone().into_iter().flat_map(TurnEvents::new)
    }
}

/// Where replay gets the wire id and start metadata of each call it opens.
trait CallSource {
    /// The next call's: `call`, opened under saved id `saved`, or `None`
    /// for a result whose call history lost.
    fn open(&mut self, saved: &str, call: Option<&model::ToolCall>) -> OpenedCall;
}

/// Opens calls as a turn saves: numbered after the session's earlier calls,
/// with what the session's tools report for them now, recorded.
struct Capture<'a> {
    tools: SessionTools<'a>,
    ids: &'a mut CallIds,
    opened: Vec<OpenedCall>,
}

impl CallSource for Capture<'_> {
    fn open(&mut self, saved: &str, call: Option<&model::ToolCall>) -> OpenedCall {
        let start = call
            .and_then(|call| {
                let tool = self.tools.tool_named(&call.name)?;
                Some(tool.start_metadata(&call.arguments))
            })
            .unwrap_or_default();
        let opened = OpenedCall {
            id: self.ids.open(saved),
            start,
        };
        self.opened.push(opened.clone());
        opened
    }
}

/// Opens the calls a [`Capture`] recorded, in order.
struct Recorded<'a> {
    calls: &'a [OpenedCall],
    next: &'a mut usize,
}

impl CallSource for Recorded<'_> {
    fn open(&mut self, _saved: &str, _call: Option<&model::ToolCall>) -> OpenedCall {
        let opened = self
            .calls
            .get(*self.next)
            .cloned()
            .expect("a saved turn renders the calls its capture opened");
        *self.next += 1;
        opened
    }
}

/// The id replay gives the first message of the display row saved at
/// `origin`: a live user message saved there takes it too, so a client sees
/// one message whether it watched the prompt live or got it replayed.
pub(super) fn saved_message_id(origin: &DisplayRowOrigin) -> MessageId {
    let mut ids = MessageIds::default();
    ids.enter(Some(origin));
    ids.next()
}

/// A replay of display rows under way: what it needs from the rows it
/// rendered to render the next.
struct Replay {
    /// Session working directory; titles show paths relative to it.
    workspace: PathBuf,
    /// The session's MCP tool names.
    mcp_tools: McpToolNames,
    /// Draws calls as the TUI draws them, for `ToolCallUpdate::display`.
    presenter: InteractiveToolPresenter,
    /// The latest call opened under each saved id, for its results.
    calls: HashMap<String, ReplayedCall>,
    /// How many calls replay opened.
    opened: usize,
    message_ids: MessageIds,
    /// Rendered since the last [`Self::take_events`].
    events: Vec<WireEvent>,
}

/// A call replay opened, and what it shows so far.
struct ReplayedCall {
    id: ToolCallId,
    /// The saved call, or a stand-in for an orphan result's, which its
    /// results' cards are drawn from.
    call: model::ToolCall,
    status: ToolStatus,
    /// Its results so far, shared with the rows: a later result's content
    /// includes theirs.
    results: Vec<Arc<ToolResult>>,
    /// How many calls replay opened before it, for closing calls in order.
    opened_at: usize,
}

impl ReplayedCall {
    fn closed(self) -> WireEvent {
        WireEvent::tool_call(status_update(self.id, Some(ToolStatus::Failed)))
    }

    /// The call's content: every result's, in order.
    fn content(&self) -> Vec<rho_protocol::ToolContent> {
        self.results
            .iter()
            .flat_map(|result| result_content(&result.content, &result.images))
            .collect()
    }
}

impl Replay {
    fn new(workspace: &Path, mcp_tools: &McpToolNames) -> Self {
        Self {
            workspace: workspace.to_owned(),
            mcp_tools: mcp_tools.clone(),
            presenter: InteractiveToolPresenter::for_saved_calls(
                workspace.to_owned(),
                mcp_tools.clone(),
            ),
            calls: HashMap::new(),
            opened: 0,
            message_ids: MessageIds::default(),
            events: Vec::new(),
        }
    }

    /// Renders `row`, opening its calls from `calls`.
    fn row(&mut self, row: &ProjectedMessage, calls: &mut dyn CallSource) {
        self.message_ids.enter(row.origin.as_ref());
        self.message(&row.message, calls);
    }

    /// Ends the replay: closes the calls the rows left open, in the order
    /// they opened, then sends `plan` when it has entries.
    fn finish(&mut self, plan: &[PlanEntry]) {
        let mut unfinished: Vec<_> = std::mem::take(&mut self.calls)
            .into_values()
            .filter(|call| call.status == ToolStatus::InProgress)
            .collect();
        unfinished.sort_by_key(|call| call.opened_at);
        self.events
            .extend(unfinished.into_iter().map(ReplayedCall::closed));
        if !plan.is_empty() {
            self.events.push(WireEvent::Plan {
                entries: plan.to_vec(),
            });
        }
    }

    fn take_events(&mut self) -> Vec<WireEvent> {
        std::mem::take(&mut self.events)
    }

    fn message(&mut self, message: &Message, calls: &mut dyn CallSource) {
        // A result is kept, shared, until its call's last result.
        if let Message::ToolResult(result) = message {
            self.result(result, calls);
            return;
        }
        match message.semantic() {
            SemanticMessage::ToolResult(_) => unreachable!("matched above"),
            SemanticMessage::User(blocks) | SemanticMessage::BoundaryInput(blocks) => {
                let content = user_content(blocks);
                if !content.is_empty() {
                    let message = self.message_ids.next();
                    self.events.push(WireEvent::UserMessage {
                        message,
                        content,
                        steer: None,
                    });
                }
            }
            SemanticMessage::Assistant(blocks) => self.agent(blocks, calls),
            SemanticMessage::EnrichedAssistant(assistant) => {
                // Reasoning streams before the text and calls it led to, so
                // it replays first too.
                if let Some(summary) = assistant
                    .reasoning_summary
                    .as_deref()
                    .filter(|summary| !summary.is_empty())
                {
                    let message = self.message_ids.next();
                    self.events.push(WireEvent::ThoughtChunk {
                        message,
                        text: summary.to_owned(),
                    });
                }
                self.agent(&assistant.content, calls);
            }
            SemanticMessage::AbortedAssistant(assistant) => self.agent(&assistant.content, calls),
            SemanticMessage::System(text) => {
                if let Some(transcript) = DisplayTranscript::from_display(text) {
                    self.events.extend(cards(&transcript));
                }
            }
            // Display rows never hold model-only entries.
            SemanticMessage::CompactionSummary(_)
            | SemanticMessage::ModelContext(_)
            | SemanticMessage::ReasoningShift(_) => {}
        }
    }

    /// An assistant message: its text, as one message per run of text
    /// between tool calls (live text after a call is a new message too), and
    /// its calls. The wire's agent messages carry text only, so images an
    /// assistant message holds are not replayed.
    fn agent(&mut self, blocks: &[model::ContentBlock], calls: &mut dyn CallSource) {
        let mut text = String::new();
        for block in blocks {
            match block {
                model::ContentBlock::Text(part) => text.push_str(part),
                model::ContentBlock::Image(_) => {}
                model::ContentBlock::ToolCall(call) => {
                    self.agent_text(std::mem::take(&mut text));
                    self.call(call, calls);
                }
            }
        }
        self.agent_text(text);
    }

    fn agent_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let message = self.message_ids.next();
        self.events
            .push(WireEvent::AgentMessageChunk { message, text });
    }

    fn call(&mut self, call: &model::ToolCall, calls: &mut dyn CallSource) {
        let OpenedCall {
            id,
            start: metadata,
        } = calls.open(&call.id, Some(call));
        // Drawn as live draws the call's start; an empty saved id cannot
        // start, so its proposal's card stands.
        let proposed = self.presenter.proposed(call.clone()).card;
        let card = rho_sdk::ToolCallId::from_string(call.id.as_str()).map_or(proposed, |call_id| {
            self.presenter
                .started(call_id, call.name.clone(), metadata.clone())
                .card
        });
        // Its result's card is drawn from the saved call.
        self.presenter.forget(&call.id);
        let opened = ToolCallUpdate {
            display: Some(Presentation::Card(card)),
            ..opened_call(
                id.clone(),
                &call.name,
                Some(call.arguments.clone()),
                &metadata,
                &self.workspace,
                &self.mcp_tools,
            )
        };
        self.open(&call.id, id, call.clone(), Vec::new(), opened);
    }

    /// A result updates the latest call opened under its id; results replace
    /// a call's content as a whole, so it keeps earlier output. An orphan
    /// result still opens a complete card.
    fn result(&mut self, result: &Arc<ToolResult>, calls: &mut dyn CallSource) {
        let ok = !result.is_failure();
        let status = if ok {
            ToolStatus::Completed
        } else {
            ToolStatus::Failed
        };
        if let Some(call) = self.calls.get_mut(&result.id) {
            call.status = status;
            call.results.push(Arc::clone(result));
            let shown = self.presenter.historical(&call.call, ok, &result.content);
            self.events.push(WireEvent::tool_call(ToolCallUpdate {
                content: Some(call.content()),
                display: Some(shown.presentation),
                ..status_update(call.id.clone(), Some(status))
            }));
            return;
        }
        let OpenedCall { id, .. } = calls.open(&result.id, None);
        // Drawn as the TUI draws a result whose call history lost.
        let call = model::ToolCall::new(
            result.id.clone(),
            "tool".into(),
            serde_json::Value::Object(Default::default()),
        );
        let shown = self.presenter.historical(&call, ok, &result.content);
        let orphan = ToolCallUpdate {
            title: Some(UNNAMED_TOOL_TITLE.to_owned()),
            kind: Some(rho_protocol::ToolKind::Other),
            content: Some(result_content(&result.content, &result.images)),
            display: Some(shown.presentation),
            ..status_update(id.clone(), Some(status))
        };
        self.open(&result.id, id, call, vec![Arc::clone(result)], orphan);
    }

    /// Sends `opened`, the card of `call` opened as `id` under `saved` with
    /// `results` so far, first closing a call still open under the same saved id, which it
    /// supersedes.
    fn open(
        &mut self,
        saved: &str,
        id: ToolCallId,
        call: model::ToolCall,
        results: Vec<Arc<ToolResult>>,
        opened: ToolCallUpdate,
    ) {
        let call = ReplayedCall {
            id,
            call,
            status: opened.status.unwrap_or(ToolStatus::InProgress),
            results,
            opened_at: self.opened,
        };
        self.opened += 1;
        if let Some(superseded) = self.calls.insert(saved.to_owned(), call) {
            if superseded.status == ToolStatus::InProgress {
                self.events.push(superseded.closed());
            }
        }
        self.events.push(WireEvent::tool_call(opened));
    }
}

/// Ids for replayed messages that stay the same each time a session loads.
///
/// A message is named after the display row it was saved in, so an id is
/// unique to one insertion, even of text saved twice. One saved row can
/// replay as several messages (text after a tool call, a reasoning
/// summary), which are numbered in order.
#[derive(Default)]
struct MessageIds {
    saved: String,
    part: u32,
}

impl MessageIds {
    /// Moves on to the row saved at `origin`. A row the load added has none
    /// and names no message of its own, so it continues the previous row.
    fn enter(&mut self, origin: Option<&DisplayRowOrigin>) {
        if let Some(DisplayRowOrigin { node, row }) = origin {
            self.saved = format!("{node}-{row}");
            self.part = 0;
        }
    }

    /// The id of the next message the current saved row replays as.
    fn next(&mut self) -> MessageId {
        let id = MessageId::new(format!("saved-{}-{}", self.saved, self.part));
        self.part += 1;
        id
    }
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
