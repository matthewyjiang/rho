use std::{
    collections::BTreeSet,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::Arc,
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    ErrorCode, SearchLocation, SessionEvent as WireEvent, SessionSearchParams,
    SessionSearchResponse,
};
use rho_sdk::model::{ContentBlock, Message, ToolCall, ToolResult};
use serde_json::json;
use tempfile::TempDir;

use crate::{
    app::{
        daemon_host::{
            jobs::{JobRegistry, JobStore},
            sessions::Sessions,
        },
        pull_requests::ScriptedForge,
        session_events::{LiveEvents, SavedTranscript},
        session_runtime::SessionTools,
    },
    session::{session_root_in, Session as StoredSession, SessionOrigin},
    tools::sdk_registry::AppToolSet,
};

/// A daemon over a Rho home of its own, and workspaces to save sessions in.
struct Daemon {
    home: TempDir,
    workspaces: TempDir,
    sessions: Arc<Sessions>,
}

impl Daemon {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
        let sessions = Sessions::new(jobs, home.path(), ScriptedForge::new());
        Self {
            home,
            workspaces: TempDir::new().unwrap(),
            sessions,
        }
    }

    /// Workspace `name`, as a session saves it.
    fn workspace(&self, name: &str) -> PathBuf {
        let path = self.workspaces.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        crate::paths::canonicalize(&path).unwrap()
    }

    /// A session from `origin` saved in `cwd` with `messages`.
    fn saved_from(&self, origin: SessionOrigin, cwd: &Path, messages: &[Message]) -> StoredSession {
        let root = session_root_in(self.home.path());
        let session = StoredSession::create_in_root_from(&root, cwd, origin).unwrap();
        for message in messages {
            session.append_message(message).unwrap();
        }
        session
    }

    /// A person's session saved in `cwd` with one user message, `text`.
    fn saved(&self, cwd: &Path, text: &str) -> StoredSession {
        self.saved_from(SessionOrigin::Interactive, cwd, &[Message::user_text(text)])
    }

    async fn search(
        &self,
        params: SessionSearchParams,
    ) -> Result<SessionSearchResponse, rho_protocol::RpcError> {
        self.sessions.search(params).await
    }

    /// Builds the index as a daemon's first search does, waiting for it.
    async fn built(&self) {
        let first = self.search(query("anything")).await.unwrap();
        assert!(first.indexing, "a cold index answers while it builds");
        self.sessions.index_build.finish();
    }

    /// The ids of the sessions `params` finds, in order.
    async fn found(&self, params: SessionSearchParams) -> Vec<String> {
        let page = self.search(params).await.unwrap();
        assert!(!page.indexing);
        ids(&page)
    }
}

fn query(query: &str) -> SessionSearchParams {
    SessionSearchParams::new(query, None, None, None)
}

fn ids(page: &SessionSearchResponse) -> Vec<String> {
    page.sessions
        .iter()
        .map(|hit| hit.session.summary.session.to_string())
        .collect()
}

// Covers: a daemon's first search must not wait for the transcripts to be
// read: it answers at once saying the index is building, and the build it
// starts makes the session findable, with no later search waiting either.
// Owner: daemon session search bootstrap.
#[tokio::test]
async fn a_cold_index_answers_at_once_and_its_build_finds_sessions() {
    let daemon = Daemon::new();
    let session = daemon.saved(&daemon.workspace("one"), "coldneedle");

    daemon.built().await;

    assert_eq!(
        daemon.found(query("coldneedle")).await,
        [session.id().to_owned()]
    );
}

// Covers: search finds the sessions the catalog lists, and only what their
// transcript shows now: words in any case, the last as a prefix while
// typing, in the asked workspace; not sessions a person never started, nor
// messages on a branch the session left. Owner: daemon session search.
#[tokio::test]
async fn search_matches_the_listed_sessions_current_branch() {
    let daemon = Daemon::new();
    let (one, two) = (daemon.workspace("one"), daemon.workspace("two"));
    daemon.built().await;
    let deploy = daemon.saved(&one, "Alpha deployment notes");
    let other = daemon.saved(&two, "alpha elsewhere");
    daemon.saved_from(
        SessionOrigin::Run,
        &one,
        &[Message::user_text("alpha from a scripted run")],
    );
    let branched = daemon.saved_from(
        SessionOrigin::Interactive,
        &one,
        &[
            Message::user_text("alpha shared"),
            Message::assistant_text("ok"),
        ],
    );
    let fork_point = branched.tree_facts().unwrap().active_leaf_id.unwrap();
    branched
        .append_message(&Message::user_text("zebraabandoned"))
        .unwrap();
    branched.set_leaf(&fork_point).unwrap();
    branched
        .append_message(&Message::user_text("keptbranch"))
        .unwrap();
    let (deploy, other, branched) = (
        deploy.id().to_owned(),
        other.id().to_owned(),
        branched.id().to_owned(),
    );

    let in_workspace = |text: &str, cwd: &Path| {
        SessionSearchParams::new(text, Some(cwd.to_path_buf()), None, None)
    };
    let cases = [
        (
            "every workspace",
            query("alpha"),
            vec![&deploy, &other, &branched],
        ),
        (
            "one workspace",
            in_workspace("alpha", &one),
            vec![&deploy, &branched],
        ),
        ("any case", in_workspace("ALPHA", &two), vec![&other]),
        // Prefixes of the stem the index keeps: "deployment" is `deploy`.
        ("last word as prefix", query("alpha deplo"), vec![&deploy]),
        ("only the last word as prefix", query("deplo alpha"), vec![]),
        ("abandoned branch", query("zebraabandoned"), vec![]),
        ("current branch", query("keptbranch"), vec![&branched]),
    ];
    let mut actual = Vec::new();
    for (name, params, _) in &cases {
        let found: BTreeSet<String> = daemon.found(params.clone()).await.into_iter().collect();
        actual.push((*name, found));
    }
    let expected: Vec<_> = cases
        .iter()
        .map(|(name, _, ids)| (*name, ids.iter().map(|id| (*id).clone()).collect()))
        .collect();
    assert_eq!(actual, expected);
}

// Covers: a client that follows next_cursor sees each matching session
// once, in the order one big page lists them, each page counting every
// match, even when a message indexed between pages moves every relevance
// score; and each excerpt's highlights slice its text at the match.
// Owner: daemon session search paging.
#[tokio::test]
async fn following_cursors_pages_through_every_match_once() {
    let daemon = Daemon::new();
    let cwd = daemon.workspace("one");
    daemon.built().await;
    for n in 0..5 {
        daemon.saved(&cwd, &format!("PageWord number {n}"));
    }
    daemon.saved(&cwd, "unrelated");
    let all = daemon
        .search(SessionSearchParams::new(
            "pageword",
            None,
            None,
            NonZeroU32::new(100),
        ))
        .await
        .unwrap();

    let mut walked = Vec::new();
    let mut totals = Vec::new();
    let mut cursor = None;
    loop {
        let page = daemon
            .search(SessionSearchParams::new(
                "pageword",
                None,
                cursor,
                NonZeroU32::new(2),
            ))
            .await
            .unwrap();
        totals.push(page.total);
        walked.extend(ids(&page));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
        if totals.len() == 1 {
            // A newer, more relevant match: it sorts ahead of the cursor, and
            // its word now in more messages, every score moves.
            daemon.saved(&cwd, "pageword pageword pageword");
        }
    }
    let highlighted: Vec<_> = all
        .sessions
        .iter()
        .flat_map(|hit| &hit.excerpts)
        .flat_map(|excerpt| {
            excerpt
                .highlights
                .iter()
                .map(|range| &excerpt.text[range.start as usize..range.end as usize])
        })
        .collect();

    assert_eq!(
        (all.total, ids(&all).len(), walked, totals, highlighted),
        (5, 5, ids(&all), vec![5, 6, 6], vec!["PageWord"; 5])
    );
}

// Covers: a request the daemon cannot serve as asked is the client's to fix,
// and says so as InvalidParams rather than an empty page or a store failure.
// Owner: daemon session search params.
#[tokio::test]
async fn malformed_searches_are_invalid_params() {
    let daemon = Daemon::new();
    let cases = [
        ("blank query", query("  \t")),
        ("query over the limit", query(&"a".repeat(1001))),
        (
            "cursor the daemon never issued",
            SessionSearchParams::new("word", None, Some("page-2".into()), None),
        ),
        (
            "limit over the page limit",
            SessionSearchParams::new("word", None, None, NonZeroU32::new(101)),
        ),
        (
            "relative workspace",
            SessionSearchParams::new("word", Some("relative".into()), None, None),
        ),
    ];
    let mut actual = Vec::new();
    for (name, params) in &cases {
        let code = daemon
            .search(params.clone())
            .await
            .map(|_| ())
            .map_err(|error| error.error_code());
        actual.push((*name, code));
    }
    let expected: Vec<_> = cases
        .iter()
        .map(|(name, _)| (*name, Err(Some(ErrorCode::InvalidParams))))
        .collect();
    assert_eq!(actual, expected);
}

/// Where each event of `events` says it shows a message or call, as a
/// search excerpt names one.
fn shown(events: impl Iterator<Item = WireEvent>) -> BTreeSet<String> {
    events
        .filter_map(|event| match event {
            WireEvent::UserMessage { message, .. }
            | WireEvent::AgentMessageChunk { message, .. }
            | WireEvent::ThoughtChunk { message, .. } => Some(format!("message {message}")),
            WireEvent::ToolCall(update) => Some(format!("tool_call {}", update.id)),
            _ => None,
        })
        .collect()
}

fn location(at: &SearchLocation) -> String {
    match at {
        SearchLocation::Message { id } => format!("message {id}"),
        SearchLocation::ToolCall { id } => format!("tool_call {id}"),
        other => panic!("no replay event names {other:?}"),
    }
}

// Covers: an excerpt's `at` must name an event an attach's replay of the
// session sends, so a client can scroll to the match: for messages, calls
// named by the provider, a provider id used twice, an unnamed call, a
// message that only calls tools, and a result whose call is missing.
// Owner: daemon session search against session replay.
#[tokio::test]
async fn every_excerpt_names_an_event_the_replay_sends() {
    let daemon = Daemon::new();
    let cwd = daemon.workspace("one");
    daemon.built().await;
    let call = |id: &str, command: &str| {
        ContentBlock::ToolCall(ToolCall::new(
            id.into(),
            "bash".into(),
            json!({ "command": command }),
        ))
    };
    let result = |result: ToolResult| Message::ToolResult(Arc::new(result));
    let rows = [
        ("tokuser", Message::user_text("tokuser")),
        (
            "tokagent",
            Message::Assistant(
                vec![
                    ContentBlock::Text("tokagent".into()),
                    call("call_1", "echo tokcallone"),
                ]
                .into(),
            ),
        ),
        (
            "tokresultone",
            result(ToolResult::succeeded("call_1", "tokresultone")),
        ),
        (
            "tokcalltwo",
            Message::Assistant(vec![call("call_1", "echo tokcalltwo")].into()),
        ),
        (
            "tokresulttwo",
            result(ToolResult::failed("call_1", "tokresulttwo")),
        ),
        (
            "tokorphan",
            result(ToolResult::succeeded("ghost", "tokorphan")),
        ),
        (
            "tokunnamed",
            Message::Assistant(vec![call("", "echo tokunnamed")].into()),
        ),
        (
            "toknamelessresult",
            result(ToolResult::succeeded("", "toknamelessresult")),
        ),
        ("tokfinal", Message::assistant_text("tokfinal")),
    ];
    let messages: Vec<_> = rows.iter().map(|(_, message)| message.clone()).collect();
    let session = daemon.saved_from(SessionOrigin::Interactive, &cwd, &messages);
    let tool_set = AppToolSet::disabled();
    let tools = || SessionTools::of_tool_set_for_tests(&tool_set);
    let mut live = LiveEvents::new(&cwd, tools(), /*configured_window*/ None);
    let mut transcript = SavedTranscript::default();
    transcript.push(
        live.replay(&session.active_display_history().unwrap(), &cwd, tools())
            .unwrap(),
    );
    let replayed = shown(transcript.events().map(|event| event.event));

    let mut named = Vec::new();
    for (word, _) in &rows {
        let page = daemon.search(query(word)).await.unwrap();
        let excerpt = &page.sessions[0].excerpts[0];
        let at = excerpt.at.as_ref().map(location);
        let in_replay = at.as_ref().is_some_and(|at| replayed.contains(at));
        named.push((*word, in_replay, excerpt.saved_at.is_some()));
    }

    let expected: Vec<_> = rows.iter().map(|(word, _)| (*word, true, true)).collect();
    assert_eq!(named, expected);
}
