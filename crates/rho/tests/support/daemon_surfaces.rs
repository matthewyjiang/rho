//! A2UI surfaces through the real daemon: the `ui` tool shows one, clients
//! get its messages on the call, and what the user does on it reaches the
//! model.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::SessionSurfaceAction, Client, IdleWake, JobState, Seq, SessionAttachResponse,
    SessionEvent, SessionId, SessionSetup, SessionSurfaceActionParams, Surfaces, ToolCallId,
    ToolContent, ToolStatus,
};
use serde_json::{json, Value};

use super::sessions::{is_state, Follower, Sandbox};

/// The provider id of the `ui` call every test here prompts.
const CALL: &str = "show";

/// A `ui` call that shows a confirmation on surface `merge`.
fn confirm(wait: bool) -> Value {
    json!({
        "id": CALL,
        "name": "ui",
        "arguments": {
            "express": r#"root = Column([Text("Merge?"), Button(Text("Merge"), action=Event("merge", {branch: "fix"}))])"#,
            "surface": "merge",
            "wait": wait,
        },
    })
}

/// The setup of a client that draws surfaces.
fn drawing() -> SessionSetup {
    SessionSetup {
        surfaces: Surfaces::Drawn,
        ..SessionSetup::default()
    }
}

/// Attaches to `session` as a client that draws surfaces.
async fn follow(sandbox: &Sandbox, session: &SessionId) -> (Follower, SessionAttachResponse) {
    Follower::attach_with_setup(sandbox, session, Vec::new(), None, drawing()).await
}

/// Searches for the `ui` tool in a turn of its own, as the model does
/// before its first call: the tool stays out of context until
/// `tool_search` loads it. Returns what the search found.
async fn search_ui(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
    follower: &mut Follower,
) -> String {
    let search = json!({
        "id": "load",
        "name": "tool_search",
        "arguments": {"query": "ui show the user a form"},
    });
    sandbox.prompt_tool_call(client, session, &search).await;
    follower.read_until(is_state(JobState::Completed)).await;
    text(&shown_of("load", &follower.events).1)
}

/// What `events` say of the `ui` call.
fn shown(events: &[(Seq, SessionEvent)]) -> (Option<ToolStatus>, Vec<ToolContent>) {
    shown_of(CALL, events)
}

/// What `events` say of call `id`: its latest status, and the content of
/// its latest update that carried any.
fn shown_of(id: &str, events: &[(Seq, SessionEvent)]) -> (Option<ToolStatus>, Vec<ToolContent>) {
    let call = ToolCallId::new(id);
    let updates = || {
        events.iter().filter_map(|(_, event)| match event {
            SessionEvent::ToolCall(update) if update.id == call => Some(update),
            _ => None,
        })
    };
    (
        updates().filter_map(|update| update.status).next_back(),
        updates()
            .filter_map(|update| update.content.clone())
            .next_back()
            .unwrap_or_default(),
    )
}

/// The surface among `content`.
fn surface(content: &[ToolContent]) -> Option<&ToolContent> {
    content
        .iter()
        .find(|content| matches!(content, ToolContent::Surface { .. }))
}

/// The text among `content`.
fn text(content: &[ToolContent]) -> String {
    content
        .iter()
        .filter_map(|content| match content {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Whether `event` gives the `ui` call status `status`.
fn call_is(status: ToolStatus) -> impl Fn(&SessionEvent) -> bool {
    move |event| {
        matches!(event, SessionEvent::ToolCall(update)
            if update.id == ToolCallId::new(CALL) && update.status == Some(status))
    }
}

/// A2UI's `action` message for the confirmation's button, whose id the
/// surface's `messages` give.
fn pressed(shown: &ToolContent) -> Value {
    let ToolContent::Surface { messages, .. } = shown else {
        panic!("expected a surface, got {shown:?}");
    };
    let button = messages[0]["createSurface"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["component"] == "Button")
        .unwrap()["id"]
        .clone();
    json!({
        "version": messages[0]["version"],
        "action": {
            "name": "merge",
            "surfaceId": "merge",
            "sourceComponentId": button,
            "timestamp": "2026-01-01T00:00:00Z",
            "context": {"branch": "fix"},
        },
    })
}

async fn act(client: &Client, session: &SessionId, action: Value) {
    client
        .request::<SessionSurfaceAction>(&SessionSurfaceActionParams {
            session: session.clone(),
            surface: "merge".to_owned(),
            action,
            data_model: None,
        })
        .await
        .unwrap();
}

// Covers: a block that does not compile must come back to the model as the
// call's error, and no client may be sent a surface for it
// Owner: daemon session
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_block_that_does_not_compile_is_the_calls_error_and_shows_nothing() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session_with_setup(&client, drawing()).await;
    let (mut follower, _) = follow(&sandbox, &session).await;
    search_ui(&sandbox, &client, &session, &mut follower).await;
    let call = json!({
        "id": CALL,
        "name": "ui",
        "arguments": {"express": "root = Nope(\"x\")"},
    });
    let searched = follower.events.len();

    sandbox.prompt_tool_call(&client, &session, &call).await;
    follower.read_until(is_state(JobState::Completed)).await;

    let (status, content) = shown(&follower.events);
    // The fixture answers with the tool result it read.
    let answer: String = follower.events[searched..]
        .iter()
        .filter_map(|(_, event)| match event {
            SessionEvent::AgentMessageChunk { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        (
            status,
            surface(&content),
            answer.starts_with("line 1, column 8: ")
        ),
        (Some(ToolStatus::Failed), None, true),
        "answer: {answer}"
    );
}

// Covers: a surface's messages are not saved, so a client that resets, and
// one that attaches once the daemon reloaded the session, must get them
// again from the saved call, the same as live
// Owner: daemon session
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_surface_reaches_clients_live_after_a_reset_and_after_a_reload() {
    let sandbox = Sandbox::new();
    let client = sandbox
        .start_daemon_with("tool-call", &["--unload-idle-after", "200ms"])
        .await;
    let session = sandbox.new_session_with_setup(&client, drawing()).await;
    let (mut live, _) = follow(&sandbox, &session).await;
    search_ui(&sandbox, &client, &session, &mut live).await;

    sandbox
        .prompt_tool_call(&client, &session, &confirm(false))
        .await;
    live.read_until(is_state(JobState::Completed)).await;
    let (live_status, live_content) = shown(&live.events);
    let (mut reset, _) = follow(&sandbox, &session).await;
    reset.read_until(call_is(ToolStatus::Completed)).await;
    let (_, reset_content) = shown(&reset.events);
    drop((live, reset));
    sandbox.wait_unloaded(&session, 1).await;
    let (mut reloaded, _) = follow(&sandbox, &session).await;
    reloaded.read_until(call_is(ToolStatus::Completed)).await;
    let (_, replayed_content) = shown(&reloaded.events);

    let live_surface = surface(&live_content).cloned();
    assert_eq!(
        (
            live_status,
            matches!(&live_surface, Some(ToolContent::Surface { surface, .. }) if surface == "merge"),
            surface(&reset_content).cloned(),
            surface(&replayed_content).cloned(),
        ),
        (
            Some(ToolStatus::Completed),
            true,
            live_surface.clone(),
            live_surface
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a waiting call must show its surface while it runs, and return
// the action a client sends for it
// Owner: daemon session
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_call_returns_the_users_action() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session_with_setup(&client, drawing()).await;
    let (mut follower, _) = follow(&sandbox, &session).await;
    search_ui(&sandbox, &client, &session, &mut follower).await;

    sandbox
        .prompt_tool_call(&client, &session, &confirm(true))
        .await;
    follower
        .read_until(|event| {
            matches!(event, SessionEvent::ToolCall(update)
                if update.status == Some(ToolStatus::InProgress)
                    && update.content.as_deref().is_some_and(|content| surface(content).is_some()))
        })
        .await;
    let (_, waiting) = shown(&follower.events);
    let action = pressed(surface(&waiting).unwrap());
    act(&client, &session, action.clone()).await;
    follower.read_until(call_is(ToolStatus::Completed)).await;

    let (_, content) = shown(&follower.events);
    assert_eq!(
        (
            serde_json::from_str::<Value>(&text(&content)).ok(),
            surface(&content),
        ),
        (Some(action["action"].clone()), surface(&waiting)),
    );
}

// Covers: an action on a surface whose call returned must start a turn on
// an idle session that wakes for results, and reach the model there
// Owner: daemon session
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_action_wakes_the_idle_session_with_the_action_for_the_model() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox
        .new_session_with_setup(
            &client,
            SessionSetup {
                idle_wake: IdleWake::Turn,
                ..drawing()
            },
        )
        .await;
    let (mut follower, _) = follow(&sandbox, &session).await;
    search_ui(&sandbox, &client, &session, &mut follower).await;
    sandbox
        .prompt_tool_call(&client, &session, &confirm(false))
        .await;
    follower.read_until(is_state(JobState::Completed)).await;
    let (_, content) = shown(&follower.events);
    let action = pressed(surface(&content).unwrap());

    act(&client, &session, action.clone()).await;
    follower.read_until(is_state(JobState::Running)).await;
    follower.read_until(is_state(JobState::Completed)).await;

    // What the delivery turn gave the model, as the session saved it.
    let delivered: Vec<Value> = sandbox
        .session_records(&session)
        .iter()
        .flat_map(|record| {
            record["transition"]["delta"]["appended_history"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|message| {
            let text = message["BoundaryInput"][0]["Text"].as_str()?;
            serde_json::from_str(text.rsplit_once("\n\n")?.1).ok()
        })
        .collect();
    assert_eq!(
        delivered,
        vec![action["action"].clone()],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a client that never asked for surfaces, as every client built
// before them, must get a session whose model cannot show one
// Owner: daemon session
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_whose_client_did_not_ask_for_surfaces_has_no_ui_tool() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    let (mut follower, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;
    search_ui(&sandbox, &client, &session, &mut follower).await;

    sandbox
        .prompt_tool_call(&client, &session, &confirm(false))
        .await;
    follower.read_until(is_state(JobState::Completed)).await;

    let (status, content) = shown(&follower.events);
    assert_eq!(
        (status, surface(&content), text(&content)),
        (
            Some(ToolStatus::Failed),
            None,
            "tool is unavailable".to_owned()
        ),
    );
}

// Covers: a client that draws nothing may follow a loaded session that has
// the `ui` tool; a call that waits for the user there must be refused, not
// wait for an answer nobody can give
// Owner: daemon session
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_call_is_refused_while_no_follower_draws_surfaces() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session_with_setup(&client, drawing()).await;
    let (mut drawer, _) = follow(&sandbox, &session).await;
    search_ui(&sandbox, &client, &session, &mut drawer).await;
    drawer.leave().await;
    let (mut follower, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;

    sandbox
        .prompt_tool_call(&client, &session, &confirm(true))
        .await;
    follower.read_until(call_is(ToolStatus::Failed)).await;

    let (status, content) = shown(&follower.events);
    assert_eq!(
        (status, surface(&content)),
        (Some(ToolStatus::Failed), None),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
