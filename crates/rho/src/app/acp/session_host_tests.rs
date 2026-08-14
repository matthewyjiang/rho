use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock as AcpContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent,
    ResourceLink, TextResourceContents,
};
use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, ImageContent as SdkImage};

use super::{
    convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd, SessionCwdError},
    ActivePromptError, PromptGate,
};

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

// Covers: prompt blocks must reach the model as typed text/image or fenced context
// Owner: acp session host
#[test]
fn prompt_content_maps_text_image_and_embedded_context() {
    let prompt = vec![
        AcpContentBlock::from("hello"),
        AcpContentBlock::Image(ImageContent::new("abc", "image/png")),
        AcpContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                "fn main() {}",
                "file:///workspace/main.rs",
            )),
        )),
        AcpContentBlock::ResourceLink(ResourceLink::new("notes", "file:///workspace/notes.md")),
    ];

    let input = user_input_from_prompt(&prompt).unwrap();
    assert_eq!(
        input.blocks(),
        [
            ContentBlock::Text("hello".into()),
            ContentBlock::Image(SdkImage {
                data: "abc".into(),
                mime_type: "image/png".into(),
            }),
            ContentBlock::Text("```resource file:///workspace/main.rs\nfn main() {}\n```".into()),
            ContentBlock::Text("```resource file:///workspace/notes.md\nnotes\n```".into()),
        ]
    );
}

// Covers: an empty prompt must not start a run
// Owner: acp session host
#[test]
fn empty_prompt_is_rejected() {
    assert!(user_input_from_prompt(&[]).is_err());
}

// Covers: ACP allows only one in-flight prompt per session
// Owner: acp session host
#[test]
fn one_active_prompt_is_rejected() {
    let gate = PromptGate::new();
    assert_eq!(gate.try_begin(), Ok(()));
    assert_eq!(gate.try_begin(), Err(ActivePromptError));
    gate.finish();
    assert_eq!(gate.try_begin(), Ok(()));
}

// Covers: session/cancel must be safe when no prompt is running
// Owner: acp session host
#[test]
fn cancel_is_idle_safe() {
    PromptGate::new().cancel();
}

// Covers: session/cancel during prompt start must still cancel the run
// Owner: acp session host
#[test]
fn cancel_during_start_marks_the_gate() {
    let gate = PromptGate::new();
    assert_eq!(gate.try_begin(), Ok(()));
    gate.cancel();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(token.is_cancelled());
}
