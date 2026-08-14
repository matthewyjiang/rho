use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock as AcpContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent,
    ResourceLink, TextResourceContents,
};
use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, ImageContent as SdkImage};

use super::{
    convert::{user_input_from_prompt, validate_session_cwd, workspace_cwd, SessionCwdError},
    PromptGate,
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

// Covers: a resource body that contains its own fence must not close ours early
// Owner: acp session host
#[test]
fn fenced_context_outgrows_backticks_in_the_body() {
    let prompt = vec![AcpContentBlock::Resource(EmbeddedResource::new(
        EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
            "```rust\nfn main() {}\n```",
            "file:///workspace/readme.md",
        )),
    ))];

    let input = user_input_from_prompt(&prompt).unwrap();
    assert_eq!(
        input.blocks(),
        [ContentBlock::Text(
            "````resource file:///workspace/readme.md\n```rust\nfn main() {}\n```\n````".into()
        )]
    );
}

// Covers: an empty prompt must not start a run
// Owner: acp session host
#[test]
fn empty_prompt_is_rejected() {
    assert!(user_input_from_prompt(&[]).is_err());
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
    gate.begin();
    gate.cancel();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(token.is_cancelled());
}

// Covers: a cancel aimed at a finished prompt must not cancel the next one
// Owner: acp session host
#[test]
fn cancel_before_the_next_prompt_does_not_leak_into_it() {
    let gate = PromptGate::new();
    gate.begin();
    gate.finish();
    gate.cancel();
    gate.begin();
    let token = rho_sdk::CancellationToken::new();
    gate.activate(token.clone());
    assert!(!token.is_cancelled());
}
