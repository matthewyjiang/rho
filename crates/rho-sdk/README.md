# rho-sdk

`rho-sdk` is the embeddable, headless agent runtime used by the Rho coding agent.
The crate is under active development toward its first stable release.

Construction is explicit and side-effect-free by default: no automatic writes to
`~/.rho`, no implicit environment reads, no credential-store access, and no
filesystem, process, or network tools unless a host opts in.

## Examples

Runnable examples live in [`examples/`](./examples):

| Example | Shows |
| --- | --- |
| `simple_completion` | Final-answer `Session::complete` |
| `streaming` | Ordered `RunEvent`s plus typed `RunOutcome` |
| `custom_provider` | Implementing `ModelProvider` |
| `custom_tool` | Implementing `Tool` and multi-step tool loops |
| `session_snapshot` | Snapshot, `InMemorySessionStore`, and restore |
| `image_history` | Image input and explicit in-memory history |
| `cancellation` | Cooperative run cancellation |
| `questionnaire_approval` | Host questionnaires and capability approvals |

```sh
cargo run -p rho-sdk --example simple_completion
cargo run -p rho-sdk --example streaming
cargo run -p rho-sdk --example custom_provider
cargo run -p rho-sdk --example custom_tool
cargo run -p rho-sdk --example session_snapshot
cargo run -p rho-sdk --example image_history
cargo run -p rho-sdk --example cancellation
cargo run -p rho-sdk --example questionnaire_approval
```

## Simple completion

```rust
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, SessionOptions,
};

# async fn example() -> Result<(), rho_sdk::Error> {
let provider = ScriptedProvider::new(
    ModelIdentity::new("scripted", "test", "model"),
    [ScriptedTurn::completed(ModelResponse::Assistant(vec![
        ContentBlock::Text("hello".into()),
    ]))],
);
let rho = Rho::builder().provider(provider).build()?;
let session = rho.session(SessionOptions::default()).await?;
let outcome = session.complete("say hello").await?;
assert_eq!(outcome.text(), "hello");
# Ok(())
# }
```

## Streaming runs

`Session::start` returns a `Run` handle. Events are ordered and buffered with
bounded backpressure. Dropping the run cancels and aborts provider or tool work.

```rust
use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Rho, RunEvent, SessionOptions, UserInput,
};

# async fn example() -> Result<(), rho_sdk::Error> {
let provider = ScriptedProvider::new(
    ModelIdentity::new("scripted", "test", "model"),
    [ScriptedTurn::streaming(
        vec![ModelEvent::OutputDelta("hi".into())],
        ModelResponse::Assistant(vec![ContentBlock::Text("hi".into())]),
    )],
);
let rho = Rho::builder().provider(provider).build()?;
let session = rho.session(SessionOptions::default()).await?;
let mut run = session.start(UserInput::text("stream")).await?;
while let Some(event) = run.next_event().await {
    if let RunEvent::AssistantTextDelta { text } = event {
        print!("{text}");
    }
}
let outcome = run.outcome().await?;
assert_eq!(outcome.text(), "hi");
# Ok(())
# }
```

## Custom providers and tools

Implement `ModelProvider` and `Tool`. Both return explicit `Send` futures and
may be used as trait objects. Tools receive only the capabilities supplied
through `ToolContext`.

```rust
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelRequest, ModelResponse, ToolSpec},
    provider::{ModelProvider, ProviderFuture},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput},
    Rho, SessionOptions,
};
use serde_json::json;

struct EchoProvider;
struct PingTool;

impl ModelProvider for EchoProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("example", "local", "echo")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "pong".into(),
            )]))
        })
    }
}

impl Tool for PingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ping".into(),
            description: "return pong".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolOutput::text("pong")) })
    }
}

# async fn example() -> Result<(), rho_sdk::Error> {
let rho = Rho::builder()
    .provider(EchoProvider)
    .tool(PingTool)
    .build()?;
let session = rho.session(SessionOptions::default()).await?;
assert_eq!(session.complete("hi").await?.text(), "pong");
# Ok(())
# }
```

## Session snapshots

`Session::snapshot` returns a versioned, JSON-serializable boundary that can be
restored through `SessionOptions::from_snapshot` without SQLite.
`InMemorySessionStore` is a concrete atomic adapter for tests and simple hosts.

```rust
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    InMemorySessionStore, Rho, SessionOptions,
};

# async fn example() -> Result<(), rho_sdk::Error> {
let store = InMemorySessionStore::new();
let identity = ModelIdentity::new("scripted", "test", "model");
let first = Rho::builder()
    .provider(ScriptedProvider::new(
        identity.clone(),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("one".into()),
        ]))],
    ))
    .build()?;
let session = first.session(SessionOptions::default()).await?;
session.complete("remember").await?;
let snapshot = session.snapshot();
store.save(snapshot.clone());

let second = Rho::builder()
    .provider(ScriptedProvider::new(
        identity,
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("two".into()),
        ]))],
    ))
    .build()?;
let restored = second
    .session(SessionOptions::from_snapshot(
        store.load(snapshot.session_id()).unwrap(),
    ))
    .await?;
assert_eq!(restored.complete("continue").await?.text(), "two");
# Ok(())
# }
```

Snapshots keep history, revision, provider identity, metadata, and exact-identity
provider replay blocks. Raw reasoning is cleared before snapshot construction and
import. Credentials and other non-replayable secrets are never snapshot fields.

## Cancellation

`Run::cancellation_handle` shares cooperative cancellation with providers and
tools. A cancelled run recovers partial assistant content into session history
and returns `Error::Cancelled`. Dropping a `Run` cancels and aborts its worker.

```rust
# use rho_sdk::{
#     model::{ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
#     provider::{ModelProvider, ProviderEventSender, ProviderFuture},
#     Error, Rho, RunEvent, SessionOptions, UserInput,
# };
# struct WaitProvider;
# impl ModelProvider for WaitProvider {
#     fn identity(&self) -> ModelIdentity {
#         ModelIdentity::new("example", "local", "wait")
#     }
#     fn send_turn<'a>(&'a self, request: ModelRequest<'a>) -> ProviderFuture<'a> {
#         Box::pin(async move {
#             request.cancellation.cancelled().await;
#             Err(rho_sdk::ProviderError::interrupted("cancelled"))
#         })
#     }
#     fn send_turn_stream<'a>(
#         &'a self,
#         request: ModelRequest<'a>,
#         events: ProviderEventSender,
#     ) -> ProviderFuture<'a> {
#         Box::pin(async move {
#             events.send(ModelEvent::OutputDelta("partial".into())).await?;
#             request.cancellation.cancelled().await;
#             Err(rho_sdk::ProviderError::interrupted("cancelled"))
#         })
#     }
# }
# async fn example() -> Result<(), rho_sdk::Error> {
let rho = Rho::builder().provider(WaitProvider).build()?;
let session = rho.session(SessionOptions::default()).await?;
let mut run = session.start(UserInput::text("work")).await?;
while let Some(event) = run.next_event().await {
    if matches!(event, RunEvent::AssistantTextDelta { .. }) {
        run.cancel();
        break;
    }
}
while run.next_event().await.is_some() {}
assert!(matches!(run.outcome().await, Err(Error::Cancelled)));
# Ok(())
# }
```

## Questionnaires and approvals

Tools can request typed host questionnaires through `ToolContext::request_host_input`.
Hosts observe `RunEvent::HostInputRequested` and answer with `Run::respond`.

Security-sensitive capabilities use `ToolContext::authorize` with a
`WorkspacePolicy` and optional `ApprovalHandler`. Approvals are deny-by-default.

See `examples/questionnaire_approval.rs` for a combined questionnaire and process
approval flow.

## Runtime behavior

A session permits one active run. `Session::complete` and `Session::start` use
the same provider and tool loop. Streaming runs produce one terminal event and
expose their final typed outcome without requiring hosts to reconstruct it from
deltas.

The SDK currently requires a Tokio runtime. Provider and tool extension points
return explicit `Send` futures and may be used as trait objects. `Rho::shutdown`
is idempotent, cancels all registered runs and compactions, and rejects new
sessions or runs. Dropping a runtime handle alone does not shut down clones, so
hosts that need coordinated teardown should call `shutdown`.

Session history is replaced under one lock only after a successful run or
explicit cancellation, so provider, tool, or persistence failure leaves the prior
revision intact. The 1.0 persistence boundary is the snapshot rather than a
public transactional store trait.

## Security defaults

The default feature set is empty. Creating an SDK runtime will not implicitly
read environment variables, access an operating-system credential store, write
to `~/.rho`, initialize a terminal or logger, check for updates, or grant tools
filesystem, process, or network access.

Capabilities such as built-in providers, SQLite persistence, keychain access,
web access, and coding tools will be introduced behind explicit adapters and
opt-in Cargo features as their public contracts are stabilized.

See [the Rho repository](https://github.com/matthewyjiang/rho) and
[the SDK tracking issue](https://github.com/matthewyjiang/rho/issues/256) for
the current roadmap.
