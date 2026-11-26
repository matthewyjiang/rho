//! A daemon session's hooks: reading its hooks report never changes them,
//! and only `rho/session/hooks/reload` does.

use std::path::Path;

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{SessionHooksReload, SessionReportRead},
    Client, HookStatus, SessionHooksReloadParams, SessionHooksReport, SessionId, SessionReport,
    SessionReportData, SessionReportParams,
};

use super::sessions::{settled_job, Sandbox};

/// A user hooks file whose one observing hook, `notify`, appends `word` to
/// `log` once each run completes.
fn hooks_file(log: &Path, word: &str) -> String {
    let script = format!("cat > /dev/null; echo {word} >> '{}'", log.display());
    format!(
        "version = 1\n\n[[hook]]\nid = \"notify\"\non = \"run_completed\"\ncommand = [\"/bin/sh\", \"-c\", {script:?}]\ntimeout = \"10s\"\n"
    )
}

/// Each listed hook as its id, status and the word its command appends.
fn listed(report: &SessionHooksReport) -> Vec<(String, HookStatus, String)> {
    report
        .hooks
        .iter()
        .map(|hook| {
            let script = hook.command.last().map(String::as_str).unwrap_or_default();
            let word = script
                .split_whitespace()
                .skip_while(|token| *token != "echo")
                .nth(1)
                .unwrap_or_default();
            (hook.id.clone(), hook.status, word.to_owned())
        })
        .collect()
}

async fn hooks_report(client: &Client, session: &SessionId) -> SessionHooksReport {
    let report = client
        .request::<SessionReportRead>(&SessionReportParams::new(
            session.clone(),
            SessionReport::Hooks,
        ))
        .await
        .expect("the hooks report reads")
        .report;
    match report {
        SessionReportData::Hooks(report) => report,
        report => panic!("not a hooks report: {report:?}"),
    }
}

async fn reload(client: &Client, session: &SessionId) -> bool {
    client
        .request::<SessionHooksReload>(&SessionHooksReloadParams::new(session.clone()))
        .await
        .expect("the reload is answered")
        .reloaded
}

/// Prompts `session`, then waits until its `notify` hook has run `runs`
/// times in all, and returns the words it appended.
async fn prompt_and_wait(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
    log: &Path,
    runs: usize,
) -> Vec<String> {
    sandbox.prompt(client, session).await.unwrap();
    settled_job(sandbox, client, session).await;
    sandbox
        .poll_until("the run_completed hook to run", async || {
            let words: Vec<String> = std::fs::read_to_string(log)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect();
            (words.len() >= runs).then_some(words)
        })
        .await
}

// Covers: reading a daemon session's hooks report after its hooks file was
// edited, or made invalid, reports the file as it is now (pending and stale
// hooks, the invalid file) and leaves the hooks the session runs as they
// were; reloading an invalid file keeps them too, and only a reload of a
// valid file changes them. Before the fix the read reloaded the session's
// hooks, and an invalid file failed it as an internal error.
// Owner: daemon session hooks report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_hooks_change_only_when_reloaded() {
    let sandbox = Sandbox::new();
    let home = sandbox.isolated.rho_home();
    let file = home.join("hooks.toml");
    let log = home.join("notified.log");
    std::fs::write(&file, hooks_file(&log, "old")).unwrap();
    let client = sandbox.start_daemon("fixed").await;
    let session = sandbox.new_session(&client).await;
    prompt_and_wait(&sandbox, &client, &session, &log, 1).await;

    std::fs::write(&file, hooks_file(&log, "new")).unwrap();
    let edited = listed(&hooks_report(&client, &session).await);
    std::fs::write(&file, "version = 1\n\n[[hook]]\nid = \"notify\"\n").unwrap();
    let invalid = hooks_report(&client, &session).await;
    let invalid = (
        invalid.invalid.as_ref().map(|file| file.path.clone()),
        listed(&invalid),
    );
    let reloaded_invalid = reload(&client, &session).await;
    let ran_unchanged = prompt_and_wait(&sandbox, &client, &session, &log, 2).await;

    std::fs::write(&file, hooks_file(&log, "new")).unwrap();
    let reloaded_valid = reload(&client, &session).await;
    let after_reload = listed(&hooks_report(&client, &session).await);
    let ran_reloaded = prompt_and_wait(&sandbox, &client, &session, &log, 3).await;

    let hook = |status, word: &str| ("user:notify".to_owned(), status, word.to_owned());
    assert_eq!(
        (
            edited,
            invalid,
            reloaded_invalid,
            ran_unchanged,
            reloaded_valid,
            after_reload,
            ran_reloaded,
        ),
        (
            vec![
                hook(HookStatus::Pending, "new"),
                hook(HookStatus::Stale, "old"),
            ],
            (Some(file.clone()), vec![hook(HookStatus::Active, "old")]),
            false,
            vec!["old".to_owned(), "old".to_owned()],
            true,
            vec![hook(HookStatus::Active, "new")],
            vec!["old".to_owned(), "old".to_owned(), "new".to_owned()],
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
