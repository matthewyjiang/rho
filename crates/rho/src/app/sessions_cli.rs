//! CLI handlers for `rho sessions list`, `rho sessions rename`, and `rho sessions rm`.

use std::{
    io::{self, IsTerminal, Write},
    path::Path,
};

use crate::{
    cli::SessionsCommand,
    session::{is_cross_project, DeleteOptions, Session, SessionSummary},
};

pub(super) fn run(command: &SessionsCommand) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    match command {
        SessionsCommand::List { all_projects } => {
            let sessions = if *all_projects {
                Session::list_all()?
            } else {
                Session::list(&cwd)?
            };
            print_session_list(&sessions, *all_projects)?;
        }
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

fn print_session_list(sessions: &[SessionSummary], all_projects: bool) -> anyhow::Result<()> {
    if sessions.is_empty() {
        if all_projects {
            println!("no saved sessions");
        } else {
            println!("no saved sessions for this workspace");
        }
        return Ok(());
    }

    let id_width = sessions
        .iter()
        .map(|session| short_id(&session.id).len())
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
        if all_projects {
            println!(
                "{:<id_width$}  {}  {}",
                short_id(&session.id),
                crate::paths::display(&session.cwd),
                title,
            );
        } else {
            println!("{:<id_width$}  {title}", short_id(&session.id));
        }
    }
    Ok(())
}

fn rename_one(cwd: &Path, id_prefix: &str, title: &str) -> anyhow::Result<()> {
    let session = resolve_candidate(cwd, id_prefix)?;
    Session::set_title(&session.cwd, &session.id, title)?;
    println!(
        "renamed session {} ({}) to {}",
        short_id(&session.id),
        crate::paths::display(&session.cwd),
        one_line(title.trim()),
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
