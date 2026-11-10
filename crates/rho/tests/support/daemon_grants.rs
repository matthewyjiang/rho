//! The grant and parent the daemon holds on each session's job (#1780),
//! through the real binary: what a session's turn may do through session
//! control is what the daemon recorded, not what its caller's environment
//! says.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionNew, SessionPrompt},
    Client, ClientEnvironment, ContentBlock, GrantAccess, GrantWorkspaces, JobListParams,
    JobSummary, SessionGrant, SessionId, SessionNewParams, SessionPromptParams, SessionSetup,
};
use serde_json::{json, Value};

use super::sessions::{settled_job, Sandbox};

/// The job of `session`'s turns.
async fn job_of(client: &Client, session: &SessionId) -> JobSummary {
    client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.session.as_ref() == Some(session))
        .expect("the daemon lists the session's job")
}

/// Opens a session in the sandbox's workspace for a client with
/// `environment`, naming `parent` and `grant`.
async fn open(
    sandbox: &Sandbox,
    client: &Client,
    environment: ClientEnvironment,
    parent: Option<&SessionId>,
    grant: Option<SessionGrant>,
) -> SessionId {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment,
            agent: None,
            setup: SessionSetup::default(),
            parent: parent.cloned(),
            grant,
        })
        .await
        .unwrap()
        .session
}

/// Runs one turn of `session` under `grant` whose shell records the
/// session-control variables its processes see, and returns them: the depth,
/// and the scope as the JSON it carries, `null` when unset.
async fn seen_in_a_turn(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
    grant: Option<SessionGrant>,
) -> (String, Value) {
    let record =
        "printf '%s\\n%s' \"${RHO_SESSION_DEPTH-unset}\" \"${RHO_SESSION_SCOPE-null}\" > grant.txt";
    let call = json!({"id": "bash", "name": "bash", "arguments": {"command": record}});
    client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            content: vec![ContentBlock::Text {
                text: call.to_string(),
            }],
            timeout_ms: None,
            environment: sandbox.client_environment(),
            idempotency_key: None,
            grant,
        })
        .await
        .unwrap();
    settled_job(sandbox, client, session).await;
    let seen = std::fs::read_to_string(sandbox.workspace.join("grant.txt")).unwrap();
    let (depth, scope) = seen.split_once('\n').expect("a depth and a scope");
    (depth.to_owned(), serde_json::from_str(scope).unwrap())
}

// Covers: the daemon records the grant a session opens with on its job and
// writes the session's variables from it, over what the client's
// environment carried; a session started naming a parent records the
// parent's job and is held to the parent's recorded grant whatever it asks
// for; a prompt's grant replaces the record's, and a prompt that names none
// from an environment that carries none leaves no limit (#1780).
// Owner: daemon job records, session grants.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_holds_each_sessions_grant_and_parent_on_its_job() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let client = sandbox.start_daemon("tool-call").await;
    let limited = SessionGrant {
        depth: 1,
        access: GrantAccess::Write,
        workspaces: GrantWorkspaces::Only(vec![sandbox.workspace.clone()]),
    };
    let unlimited = SessionGrant {
        depth: 0,
        access: GrantAccess::Write,
        workspaces: GrantWorkspaces::Any,
    };
    // What the caller's environment claims loses to the grant it names.
    let claiming_deeper = sandbox
        .client_environment()
        .into_iter()
        .chain([("RHO_SESSION_DEPTH".to_owned(), "7".to_owned())])
        .collect();

    let parent = open(
        &sandbox,
        &client,
        claiming_deeper,
        None,
        Some(limited.clone()),
    )
    .await;
    let opened = job_of(&client, &parent).await;
    let child = open(
        &sandbox,
        &client,
        sandbox.client_environment(),
        Some(&parent),
        Some(unlimited),
    )
    .await;
    let started = job_of(&client, &child).await;
    let under_the_grant = seen_in_a_turn(&sandbox, &client, &parent, Some(limited.clone())).await;
    let prompted = job_of(&client, &parent).await.grant;
    let by_a_person = seen_in_a_turn(&sandbox, &client, &parent, None).await;
    let afterwards = job_of(&client, &parent).await.grant;

    let scope = json!({"access": "write", "workspaces": {"only": [sandbox.workspace]}});
    assert_eq!(
        (
            (opened.parent.clone(), opened.grant),
            (started.parent, started.grant),
            (under_the_grant, prompted),
            (by_a_person, afterwards),
        ),
        (
            (None, Some(limited.clone())),
            (
                Some(opened.id),
                Some(SessionGrant {
                    depth: 2,
                    ..limited.clone()
                })
            ),
            (("1".to_owned(), scope), Some(limited)),
            (("unset".to_owned(), Value::Null), None),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
