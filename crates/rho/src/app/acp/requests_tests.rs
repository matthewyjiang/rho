use std::{collections::BTreeSet, path::PathBuf};

use agent_client_protocol::schema::v1::{
    AudioContent, BlobResourceContents, ContentBlock as AcpContentBlock, EmbeddedResource,
    EmbeddedResourceResource, EnvVariable, ImageContent, McpServer as AcpMcpServer,
    McpServerHttp, McpServerStdio, ResourceLink, TextContent, TextResourceContents,
};
use pretty_assertions::assert_eq;
use rho_protocol::{
    ContentBlock, McpServer, SessionId, SessionListResponse, SessionPart, SessionSetup,
    SessionSummary,
};
use serde_json::json;

use super::{prompt_content, session_list, Launch};
use crate::app::prompt_content::fence_context;

// Covers: each ACP prompt block reaches the daemon as the wire block of its
// kind, and blocks the wire has no kind for as fenced text the model still
// sees
// Owner: ACP request mapping
#[test]
fn prompt_blocks_become_wire_content() {
    let blob = BlobResourceContents::new("AAAA", "file:///logo.png").mime_type("image/png");
    let cases = [
        (
            AcpContentBlock::Text(TextContent::new("hello")),
            ContentBlock::Text {
                text: "hello".into(),
            },
        ),
        (
            AcpContentBlock::Image(ImageContent::new("iVBOR", "image/png")),
            ContentBlock::Image {
                mime_type: "image/png".into(),
                data: "iVBOR".into(),
            },
        ),
        (
            AcpContentBlock::ResourceLink(
                ResourceLink::new("notes", "file:///notes.md").description("Meeting notes"),
            ),
            ContentBlock::ResourceLink {
                uri: "file:///notes.md".into(),
                name: "notes".into(),
                description: Some("Meeting notes".into()),
            },
        ),
        (
            AcpContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                    "fn main() {}",
                    "file:///main.rs",
                )),
            )),
            ContentBlock::Resource {
                uri: "file:///main.rs".into(),
                text: "fn main() {}".into(),
            },
        ),
        (
            AcpContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::BlobResourceContents(blob),
            )),
            ContentBlock::Text {
                text: fence_context("resource", "file:///logo.png image/png", "AAAA"),
            },
        ),
        (
            AcpContentBlock::Audio(AudioContent::new("UklGR", "audio/wav")),
            ContentBlock::Text {
                text: fence_context("audio", "audio/wav", "UklGR"),
            },
        ),
    ];
    for (block, expected) in cases {
        assert_eq!(prompt_content(&[block.clone()]), [expected], "{block:?}");
    }
}

// Covers: every session gets the launch flags' directories and parts, the
// host's directories after them without repeats, its MCP servers but those
// Rho cannot speak, and no questionnaire when the host draws no forms
// Owner: ACP request mapping
#[test]
fn launch_flags_and_host_request_make_the_session_setup() {
    let launch = Launch {
        agent: Some("reviewer".into()),
        added_dirs: vec![PathBuf::from("/shared")],
        without: BTreeSet::from([SessionPart::Tools]),
    };
    let servers = [
        AcpMcpServer::Stdio(
            McpServerStdio::new("files", "/bin/files")
                .args(vec!["--root".into()])
                .env(vec![
                    EnvVariable::new("TOKEN", "old"),
                    EnvVariable::new("TOKEN", "new"),
                ]),
        ),
        AcpMcpServer::Http(McpServerHttp::new("search", "https://search.example")),
    ];
    let directories = [PathBuf::from("/shared"), PathBuf::from("/docs")];

    let with_forms = launch.setup(&directories, &servers, /*forms*/ true);
    let without_forms = launch.setup(&[], &[], /*forms*/ false);

    assert_eq!(
        with_forms,
        SessionSetup {
            additional_directories: vec![PathBuf::from("/shared"), PathBuf::from("/docs")],
            mcp_servers: vec![
                McpServer::Stdio {
                    name: "files".into(),
                    command: "/bin/files".into(),
                    args: vec!["--root".into()],
                    env: [("TOKEN".to_owned(), "new".to_owned())].into_iter().collect(),
                },
                McpServer::Http {
                    name: "search".into(),
                    url: "https://search.example".into(),
                    headers: [].into_iter().collect(),
                },
            ],
            without: BTreeSet::from([SessionPart::Tools]),
        }
    );
    assert_eq!(
        without_forms.without,
        BTreeSet::from([SessionPart::Tools, SessionPart::Questions])
    );
}

// Covers: a listed session is labelled with its title, else its first user
// message, with its last save in RFC 3339, and the next page's cursor
// Owner: ACP request mapping
#[test]
fn listed_sessions_carry_label_time_and_cursor() {
    let summary = |id: &str, title: Option<&str>| SessionSummary {
        session: SessionId::new(id),
        cwd: "/work".into(),
        added_dirs: vec!["/docs".into()],
        title: title.map(str::to_owned),
        first_user_message: Some("fix the build".into()),
        last_user_message: None,
        updated_at: 1_700_000_000,
    };
    let listed = session_list(SessionListResponse {
        sessions: vec![summary("a", Some("Build fix")), summary("b", None)],
        next_cursor: Some("page-2".into()),
    });

    assert_eq!(
        serde_json::to_value(listed).unwrap(),
        json!({
            "sessions": [
                {
                    "sessionId": "a",
                    "cwd": "/work",
                    "additionalDirectories": ["/docs"],
                    "title": "Build fix",
                    "updatedAt": "2023-11-14T22:13:20Z",
                },
                {
                    "sessionId": "b",
                    "cwd": "/work",
                    "additionalDirectories": ["/docs"],
                    "title": "fix the build",
                    "updatedAt": "2023-11-14T22:13:20Z",
                },
            ],
            "nextCursor": "page-2",
        })
    );
}
