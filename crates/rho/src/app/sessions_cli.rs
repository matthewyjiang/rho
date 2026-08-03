//! CLI handlers for `rho sessions list`, `export`, `rename`, and `rm`.

use std::{
    io::{self, IsTerminal, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{
    cli::SessionsCommand,
    export::{self, ExportWriteOptions},
    session::{is_cross_project, DeleteOptions, Session, SessionSummary},
};

pub(super) fn run(command: &SessionsCommand) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    match command {
        SessionsCommand::List {
            all_projects,
            search,
            limit,
            json,
        } => {
            let mut sessions = if *all_projects {
                Session::list_all()?
            } else {
                Session::list(&cwd)?
            };
            if let Some(query) = search.as_deref() {
                sessions.retain(|session| session_matches(session, query));
            }
            if let Some(limit) = *limit {
                sessions.truncate(limit.get());
            }
            print_session_list(&sessions, *all_projects, *json)?;
        }
        SessionsCommand::Export {
            id_prefix,
            output,
            format,
            force,
        } => export_one(&cwd, id_prefix, output.as_deref(), *format, *force)?,
        SessionsCommand::Rm { ids, force, yes } => {
            for id in ids {
                delete_one(&cwd, id, *force, *yes)?;
            }
        }
        SessionsCommand::Rename { id_prefix, title } => {
            rename_one(&cwd, id_prefix, &title.join(" "))?;
        }
    }
    Ok(())
}

fn export_one(
    cwd: &Path,
    id_prefix: &str,
    output: Option<&Path>,
    format: Option<export::ExportFormat>,
    force: bool,
) -> anyhow::Result<()> {
    let path_arg = output
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let path = export::write_session_export(
        cwd,
        id_prefix,
        &ExportWriteOptions {
            path_arg: &path_arg,
            format,
            force,
        },
    )?;
    println!("exported session transcript to {}", path.display());
    Ok(())
}

#[derive(Serialize)]
struct SessionListDocument<'a> {
    sessions: Vec<SessionListItem<'a>>,
}

#[derive(Serialize)]
struct SessionListItem<'a> {
    id: &'a str,
    cwd: String,
    created_at: u64,
    updated_at: u64,
    message_count: u64,
    title: Option<&'a str>,
    first_user_message: Option<&'a str>,
    last_user_message: Option<&'a str>,
}

fn print_session_list(
    sessions: &[SessionSummary],
    all_projects: bool,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        let document = SessionListDocument {
            sessions: sessions
                .iter()
                .map(|session| SessionListItem {
                    id: &session.id,
                    cwd: crate::paths::display(&session.cwd),
                    created_at: session.created_at,
                    updated_at: session.updated_at,
                    message_count: session.message_count,
                    title: session.title.as_deref(),
                    first_user_message: session.first_user_message.as_deref(),
                    last_user_message: session.last_user_message.as_deref(),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    if sessions.is_empty() {
        if all_projects {
            println!("no saved sessions");
        } else {
            println!("no saved sessions for this workspace");
        }
        return Ok(());
    }

    let now = now_unix_secs();
    let id_width = sessions
        .iter()
        .map(|session| short_id(&session.id).len())
        .max()
        .unwrap_or(8)
        .max(8);
    let updated_width = sessions
        .iter()
        .map(|session| format_updated_ago(session.updated_at, now).len())
        .max()
        .unwrap_or(8)
        .max(8);

    for session in sessions {
        let title = session
            .title
            .as_deref()
            .or(session.first_user_message.as_deref())
            .map(one_line)
            .unwrap_or_else(|| "(untitled)".into());
        let updated = format_updated_ago(session.updated_at, now);
        if all_projects {
            println!(
                "{:<id_width$}  {:<updated_width$}  {}  {}",
                short_id(&session.id),
                updated,
                crate::paths::display(&session.cwd),
                title,
            );
        } else {
            println!(
                "{:<id_width$}  {:<updated_width$}  {title}",
                short_id(&session.id),
                updated,
            );
        }
    }
    Ok(())
}

fn session_matches(session: &SessionSummary, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    let contains = |text: &str| text.to_ascii_lowercase().contains(&query);
    if contains(&session.id)
        || session.title.as_deref().is_some_and(contains)
        || session.first_user_message.as_deref().is_some_and(contains)
        || session.last_user_message.as_deref().is_some_and(contains)
    {
        return true;
    }
    contains(&crate::paths::display(&session.cwd))
}

fn rename_one(cwd: &Path, id_prefix: &str, title: &str) -> anyhow::Result<()> {
    let updated = Session::set_title(cwd, id_prefix, title)?;
    println!(
        "renamed session {} ({}) to {}",
        short_id(&updated.id),
        crate::paths::display(&updated.cwd),
        one_line(&updated.title),
    );
    Ok(())
}

fn delete_one(cwd: &Path, id_prefix: &str, force: bool, yes: bool) -> anyhow::Result<()> {
    // Resolve first so cross-project confirmation can show the real cwd before
    // any destructive work. delete_by_id resolves again; that is intentional so
    // the confirmation path stays a pure preview.
    let session = resolve_candidate(cwd, id_prefix)?;

    if is_cross_project(&session.cwd, cwd) && !yes {
        confirm_cross_project(&session)?;
    }

    let outcome = Session::delete_by_id(
        cwd,
        id_prefix,
        DeleteOptions {
            force,
            protect_session_id: None,
        },
    )?;

    if !outcome.forced_run_ids.is_empty() {
        eprintln!(
            "warning: force-deleted non-terminal run(s): {}",
            outcome.forced_run_ids.join(", ")
        );
    }

    println!(
        "deleted session {} ({}){}",
        short_id(&outcome.id),
        crate::paths::display(&outcome.cwd),
        if outcome.deleted_run_count > 0 {
            format!(
                " and {} related run{}",
                outcome.deleted_run_count,
                if outcome.deleted_run_count == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            String::new()
        }
    );
    Ok(())
}

fn resolve_candidate(cwd: &Path, id_prefix: &str) -> anyhow::Result<SessionSummary> {
    let preview = Session::list_all()?
        .into_iter()
        .filter(|session| session.id.starts_with(id_prefix))
        .collect::<Vec<_>>();
    // Prefer the same resolution rules as delete (local workspace first).
    let local = Session::list(cwd)?
        .into_iter()
        .filter(|session| session.id.starts_with(id_prefix))
        .collect::<Vec<_>>();
    let candidates = if local.is_empty() { preview } else { local };
    match candidates.as_slice() {
        [] => anyhow::bail!("no session found matching '{id_prefix}'"),
        [only] => Ok(only.clone()),
        many => {
            let mut detail = many
                .iter()
                .map(|session| {
                    format!(
                        "  {}  {}",
                        short_id(&session.id),
                        crate::paths::display(&session.cwd)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            if detail.is_empty() {
                detail = "(no details)".into();
            }
            anyhow::bail!(
                "multiple sessions match '{id_prefix}'; use a longer UUID prefix\n{detail}"
            );
        }
    }
}

fn confirm_cross_project(session: &SessionSummary) -> anyhow::Result<()> {
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "session {} belongs to workspace {}; pass --yes to delete cross-project sessions without a prompt",
            short_id(&session.id),
            crate::paths::display(&session.cwd)
        );
    }
    eprint!(
        "Delete session {} from workspace {}? [y/N] ",
        short_id(&session.id),
        crate::paths::display(&session.cwd)
    );
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim();
    if !answer.eq_ignore_ascii_case("y") && !answer.eq_ignore_ascii_case("yes") {
        anyhow::bail!("delete cancelled");
    }
    Ok(())
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn one_line(text: &str) -> String {
    let text = text.replace('\n', " ");
    if text.chars().count() <= 72 {
        return text;
    }
    let mut preview = text.chars().take(71).collect::<String>();
    preview.push('…');
    preview
}

fn format_updated_ago(updated_at: u64, now: u64) -> String {
    let age_secs = now.saturating_sub(updated_at);
    if age_secs < 60 {
        return format!("{age_secs}s ago");
    }
    let minutes = age_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    // Covers: list search matches id/title/message and ignores unrelated rows.
    // Owner: pure unit
    #[test]
    fn session_search_filters_id_title_and_messages() {
        let session = SessionSummary {
            id: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".into(),
            path: PathBuf::from("/tmp/session"),
            cwd: PathBuf::from("/tmp/workspace"),
            created_at: 1,
            updated_at: 2,
            message_count: 3,
            title: Some("Fix login".into()),
            first_user_message: Some("please fix auth".into()),
            last_user_message: Some("retry".into()),
        };
        assert!(session_matches(&session, "aaaa"));
        assert!(session_matches(&session, "LOGIN"));
        assert!(session_matches(&session, "auth"));
        assert!(session_matches(&session, "workspace"));
        assert!(!session_matches(&session, "billing"));
    }
}
