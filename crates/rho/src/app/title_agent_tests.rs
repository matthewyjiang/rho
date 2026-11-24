use pretty_assertions::assert_eq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{generate_title, TitleRequest};
use crate::{config::Config, session_environment::SessionEnvironment};

/// One streamed Chat Completions reply: a call of `read_file`, or text.
enum Reply {
    ReadFile(&'static str),
    Text(&'static str),
}

/// Answers one Chat Completions request per reply, in order, and yields
/// each request it answered, whole.
async fn serve(listener: tokio::net::TcpListener, replies: Vec<Reply>) -> Vec<String> {
    let mut requests = Vec::new();
    for reply in replies {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 16_384];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            if request.len() >= header_end + 4 + length {
                break;
            }
        }
        let chunk = |delta: serde_json::Value, finish: Option<&str>| {
            let chunk = serde_json::json!({
                "id": "title",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "local-model",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
            });
            format!("data: {chunk}\n\n")
        };
        let (delta, finish) = match reply {
            Reply::ReadFile(path) => (
                serde_json::json!({"role": "assistant", "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": serde_json::json!({"path": path}).to_string(),
                    },
                }]}),
                "tool_calls",
            ),
            Reply::Text(text) => (
                serde_json::json!({"role": "assistant", "content": text}),
                "stop",
            ),
        };
        let body = [
            chunk(delta, None),
            chunk(serde_json::json!({}), Some(finish)),
            "data: [DONE]\n\n".to_owned(),
        ]
        .concat();
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        );
        stream.write_all(reply.as_bytes()).await.unwrap();
        requests.push(String::from_utf8_lossy(&request).into_owned());
    }
    requests
}

/// A workspace holding `plan.md`, a title agent on `config`'s custom
/// provider, and the server that provider reaches, which answers with a
/// read of `plan.md` and then a title.
struct Fixture {
    workspace: tempfile::TempDir,
    config: Config,
    server: tokio::task::JoinHandle<Vec<String>>,
}

const PLAN: &str = "Migrate the billing tables to the ledger schema";
const TITLE: &str = "Migrate billing tables to ledger";

impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Config {
            provider: "titlehost".into(),
            model: "local-model".into(),
            auth: rho_providers::provider::KEYLESS_AUTH.into(),
            ..Config::default()
        };
        config
            .providers
            .set_endpoint(
                "titlehost",
                &format!("http://{}/v1", listener.local_addr().unwrap()),
            )
            .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("plan.md"), PLAN).unwrap();
        let server = tokio::spawn(serve(
            listener,
            vec![Reply::ReadFile("plan.md"), Reply::Text(TITLE)],
        ));
        Self {
            workspace,
            config,
            server,
        }
    }

    /// Names "implement plan.md" in `environment`, and yields the title
    /// with each request the title model got.
    async fn title(self, environment: SessionEnvironment) -> (String, Vec<String>) {
        let title = generate_title(TitleRequest {
            config: self.config,
            config_path: self.workspace.path().join("config.toml"),
            cwd: self.workspace.path().to_path_buf(),
            added_dirs: Default::default(),
            environment,
            session: rho_sdk::SessionId::new(),
            input: crate::title::session_input(&[rho_sdk::model::ContentBlock::Text(
                "implement plan.md".into(),
            )]),
        })
        .await
        .unwrap();
        (title, self.server.await.unwrap())
    }
}

// Covers: the title agent reads a workspace file its material points at
// before it names the work, on the title model the config resolves to: a
// custom provider from `[providers]` here, which a title model built
// without the config's providers never reached. Its request offers the
// read-only tools and the two orchestration tools every session has, and
// nothing that writes or runs.
// Owner: title agent run
#[tokio::test]
async fn the_title_agent_reads_the_workspace_on_the_configured_model() {
    let (title, requests) = Fixture::new().await.title(Default::default()).await;

    let (head, body) = requests[0].split_once("\r\n\r\n").unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    let mut tools: Vec<_> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    tools.sort_unstable();
    assert_eq!(
        (
            title.as_str(),
            requests.len(),
            head.lines().next().unwrap(),
            tools,
            requests[1].contains(PLAN),
        ),
        (
            TITLE,
            2,
            "POST /v1/chat/completions HTTP/1.1",
            vec![
                "codemode",
                "glob",
                "grep",
                "list_dir",
                "read_file",
                "tool_search"
            ],
            true,
        )
    );
}

// Covers: the title agent runs unattended on every first prompt, so a
// `before_tool_use` hook that denies a read must stop its read too: what
// the hook guards never reaches the title model.
// Owner: title agent run
#[cfg(unix)]
#[tokio::test]
async fn a_hook_that_denies_a_read_stops_the_title_agents_read() {
    use crate::workspace::{ProjectTrust, ProjectTrusts};

    let fixture = Fixture::new().await;
    let hooks = fixture.workspace.path().join(".rho");
    std::fs::create_dir(&hooks).unwrap();
    std::fs::write(
        hooks.join("hooks.toml"),
        r#"
version = 1

[[hook]]
id = "deny-reads"
on = "before_tool_use"
tools = ["read_file"]
command = ["/bin/sh", "-c", "echo '{\"version\":1,\"decision\":\"deny\",\"reason\":\"guarded\"}'"]
timeout = "10s"
"#,
    )
    .unwrap();

    let (title, requests) = fixture
        .title(SessionEnvironment::for_reading(ProjectTrusts {
            hooks: ProjectTrust::Trusted,
            ..ProjectTrusts::UNTRUSTED
        }))
        .await;

    assert_eq!(
        (title.as_str(), requests.len(), requests[1].contains(PLAN)),
        (TITLE, 2, false)
    );
}
