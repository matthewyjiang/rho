use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    AudioContent, BlobResourceContents, ContentBlock as AcpContentBlock, EmbeddedResource,
    EmbeddedResourceResource, SessionConfigKind, SessionConfigOption,
};
use pretty_assertions::assert_eq;
use rho_sdk::model::ContentBlock;

use super::{
    advertised_config_options,
    convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd, SessionCwdError},
    setting_error, CurrentModel, CurrentSettings, Toggles,
};
use crate::{app::session_runtime::SettingChangeError, config::Config, permission::PermissionMode};
use rho_providers::reasoning::ReasoningLevel;

// Covers: session/new and session/load must refuse a non-workspace cwd
// Owner: acp session host
#[test]
fn session_cwd_must_be_an_absolute_existing_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let file = dir.join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let missing = dir.join("missing");

    let cases: &[(&str, PathBuf, Result<(), SessionCwdError>)] = &[
        (
            "relative",
            PathBuf::from("relative"),
            Err(SessionCwdError::NotAbsolute),
        ),
        ("missing", missing, Err(SessionCwdError::NotDirectory)),
        ("file", file, Err(SessionCwdError::NotDirectory)),
        ("directory", dir, Ok(())),
    ];

    for (label, path, expected) in cases {
        assert_eq!(validate_session_cwd(path).map(drop), *expected, "{label}");
    }
}

// Covers: session/load must fall back to the process cwd when the request cwd is empty
// Owner: acp session host
#[test]
fn load_workspace_prefers_request_cwd_then_process_cwd() {
    let process = Path::new("/process");
    let requested = Path::new("/requested");
    assert_eq!(workspace_cwd(Path::new(""), process), process);
    assert_eq!(workspace_cwd(requested, process), requested);
}

// Covers: ACP blocks the daemon wire has no kind for reach the model as
// fenced text: audio under its mime type, and a binary resource under its
// URI and mime type, if any. Shared blocks convert in `prompt_content`.
// Owner: acp session host
#[test]
fn acp_only_prompt_blocks_reach_the_model_as_fenced_text() {
    let blob = || BlobResourceContents::new("AAEC", "file:///workspace/a.bin");
    let cases = [
        (
            "audio",
            AcpContentBlock::Audio(AudioContent::new("UklG", "audio/wav")),
            "```audio audio/wav\nUklG\n```",
        ),
        (
            "a binary resource",
            AcpContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::BlobResourceContents(
                    blob().mime_type("application/octet-stream"),
                ),
            )),
            "```resource file:///workspace/a.bin application/octet-stream\nAAEC\n```",
        ),
        (
            "a binary resource without a mime type",
            AcpContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::BlobResourceContents(blob()),
            )),
            "```resource file:///workspace/a.bin\nAAEC\n```",
        ),
    ];
    for (name, block, expected) in cases {
        let input = user_input_from_prompt(&[block]).unwrap();
        assert_eq!(
            input.blocks(),
            [ContentBlock::Text(expected.into())],
            "{name}"
        );
    }
}

// Covers: session/new and set_config_option (including after a model switch)
// must advertise the permission mode first (spec clients ignore legacy
// `modes` once config options exist), then model, then thought_level only
// when the current model can change reasoning, then fast_mode only when it
// implements fast mode.
// Owner: acp session config mapper
#[test]
fn advertised_options_merge_model_with_per_model_options() {
    let cases = [
        (
            CurrentModel {
                provider: "test".into(),
                model: "model".into(),
                auth: "api-key".into(),
            },
            ["mode", "model", "thought_level"].as_slice(),
        ),
        (
            CurrentModel {
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                auth: "codex".into(),
            },
            ["mode", "model", "thought_level", "fast_mode"].as_slice(),
        ),
        (
            CurrentModel {
                provider: "github-copilot".into(),
                model: "gpt-4.1".into(),
                auth: "github-copilot".into(),
            },
            ["mode", "model"].as_slice(),
        ),
    ];
    for (current, expected_ids) in cases {
        let config = Config {
            provider: current.provider.clone(),
            model: current.model.clone(),
            auth: current.auth.clone(),
            ..Config::default()
        };
        let settings = CurrentSettings {
            permission_mode: PermissionMode::AllowEdits,
            reasoning: ReasoningLevel::High,
            fast: true,
        };
        let options = advertised_config_options(&current, &config, settings, Toggles::Select);
        let ids = options
            .iter()
            .map(|option| option.id.0.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected_ids, "{}", current.provider);
        assert_eq!(
            select_current(&options[0]),
            "allow_edits",
            "{}",
            current.provider
        );
        if let Some(thought) = options
            .iter()
            .find(|option| option.id.0.as_ref() == "thought_level")
        {
            assert_eq!(select_current(thought), "high", "{}", current.provider);
        }
    }
}

fn select_current(option: &SessionConfigOption) -> &str {
    match &option.kind {
        SessionConfigKind::Select(select) => select.current_value.0.as_ref(),
        other => panic!("expected a select option, got {other:?}"),
    }
}

// Covers: a refused setting change (permission mode, model, thought level,
// or fast mode) must reach the client as the ACP error it can act on: fix
// the request, resend once the turn ends, or report the agent's failure.
// Owner: acp session host
#[test]
fn setting_errors_map_to_acp_errors() {
    let session_id = agent_client_protocol::schema::v1::SessionId::new("session-1");
    let cases = [
        (
            SettingChangeError::Unsupported("supported levels: low, high".into()),
            agent_client_protocol::Error::invalid_params().data("supported levels: low, high"),
        ),
        (
            SettingChangeError::Busy,
            agent_client_protocol::Error::invalid_request()
                .data("session 'session-1' already has an active prompt"),
        ),
        (
            SettingChangeError::Apply(anyhow::anyhow!("provider refused")),
            agent_client_protocol::Error::internal_error().data("provider refused"),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(setting_error(&session_id, error), expected);
    }
}
