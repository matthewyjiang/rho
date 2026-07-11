use std::path::PathBuf;

use crate::{
    agent::{Agent, SessionHistorySink},
    cli::Cli,
    config::Config,
    herdr::HerdrReporter,
    session::Session,
    tui::{self, TuiInfo},
};

use super::config_repository::ConfigRepository;

pub(super) struct Startup<'a> {
    pub(super) cli: &'a Cli,
    pub(super) config: Config,
    pub(super) config_repository: ConfigRepository,
    pub(super) cwd: PathBuf,
    pub(super) missing_auth_error: Option<String>,
    pub(super) update_notice: Option<String>,
    pub(super) herdr: HerdrReporter,
}

pub(super) async fn run(agent: &mut Agent, startup: Startup<'_>) -> anyhow::Result<()> {
    let Startup {
        cli,
        config,
        config_repository,
        cwd,
        missing_auth_error,
        update_notice,
        herdr,
    } = startup;
    let mut open_resume_picker = false;
    let mut recovered_messages = Vec::new();
    let session_id = match &cli.resume {
        Some(Some(id)) => {
            let (session, histories) = Session::open_by_id_with_histories(&cwd, id)?;
            let session_id = Some(session.id().to_string());
            recovered_messages = histories.display;
            agent.replace_history(histories.model);
            agent.set_session_id(session_id.clone());
            agent.set_history_sink(SessionHistorySink::new(session));
            session_id
        }
        Some(None) => {
            open_resume_picker = true;
            None
        }
        None => None,
    };
    let mut prompt_templates = crate::prompt_templates::discover(&cwd);
    crate::prompt_templates::merge(&mut prompt_templates, config.prompt_templates);
    let tui_result = tui::run(
        agent,
        TuiInfo {
            cwd,
            provider: config.provider,
            model: config.model,
            reasoning: config.reasoning,
            show_reasoning_output: config.show_reasoning_output,
            auth: config.auth,
            title_provider: config.title_provider,
            title_model: config.title_model,
            title_auth: config.title_auth,
            favorite_models: config.favorite_models,
            max_tool_output_lines: config.max_tool_output_lines,
            keybindings: config.keybindings,
            prompt_templates,
            questionnaire_enabled: !cli.no_tools,
            session_id,
            recovered_messages,
            open_resume_picker,
            config_repository,
            auth_unavailable: missing_auth_error,
            update_notice,
            herdr,
        },
    )
    .await?;
    if let Some(session_id) = tui_result.resume_session_id {
        println!("\nResume this session:\n  rho --resume {session_id}\n");
    }
    Ok(())
}
