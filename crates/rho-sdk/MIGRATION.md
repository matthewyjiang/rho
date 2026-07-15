# Migrating to `rho-sdk`

This guide covers Rust code that previously imported private modules from the
`rho-coding-agent` application crate. The application internals are not a stable
library API. Depend on the published `rho-sdk` crate instead.

## Runtime and session ownership

Replace direct construction of `Agent`, `AppBuilder`, global configuration, or
TUI state with explicit SDK ownership:

```rust
let rho = rho_sdk::Rho::builder()
    .provider(my_provider)
    .build()?;
let session = rho.session(rho_sdk::SessionOptions::default()).await?;
let outcome = session.complete("hello").await?;
```

`Rho` owns immutable runtime configuration and a coordinated shutdown
lifecycle. `Session` owns provider-neutral history and permits one mutable run.
`Run` owns streaming work. Call `Rho::shutdown` for coordinated teardown;
dropping an unfinished `Run` cancels and aborts that worker as a fallback.

## Streaming and callbacks

Replace application callback traits and mutable callback references with the
bounded event stream returned by `Session::start`:

```rust
let mut run = session.start(rho_sdk::UserInput::text("hello")).await?;
while let Some(event) = run.next_event().await {
    match event {
        rho_sdk::RunEvent::AssistantTextDelta { text } => print!("{text}"),
        rho_sdk::RunEvent::HostInputRequested { request } => {
            // Present the structured questionnaire and call run.respond(...).
            println!("host input required: {}", request.title());
        }
        _ => {}
    }
}
let outcome = run.outcome().await?;
```

Events are ordered and backpressured. Do not reconstruct the final answer,
usage, or revision from deltas; use `Run::outcome`.

## Providers

Implement `rho_sdk::provider::ModelProvider` instead of importing an
application provider trait. Implementations return the explicit `Send`
`ProviderFuture`, receive a provider-neutral `ModelRequest`, and stream through
a bounded `ProviderEventSender`. Scope opaque replay blocks to the exact
`ModelIdentity` that produced them.

Credential discovery is not an SDK default. Resolve environment variables,
files, keychains, or interactive login in a host adapter, then pass the
credential explicitly to the provider implementation. Never include a secret in
`Debug`, errors, events, diagnostics, snapshots, or model history.

See [`examples/custom_provider.rs`](examples/custom_provider.rs).

## Tools

Implement `rho_sdk::tool::Tool` instead of importing application tool modules.
A tool receives owned `ToolInvocation` data and `ToolContext`. Use
`ToolContext::authorize` before filesystem, process, or network work and report
structured paths, commands, URLs, and diffs through `ToolMetadata`.

The minimal SDK does not install coding tools. Rho's built-in coding tools stay
in the application companion module so SDK consumers do not acquire shell, web,
SQLite, or keychain dependencies. Register only the tools your host intends to
grant.

See [`examples/custom_tool.rs`](examples/custom_tool.rs) and
[`examples/questionnaire_approval.rs`](examples/questionnaire_approval.rs).

## History and persistence

Do not mutate internal message vectors. Initialize history with
`SessionOptions::history`, inspect it with `Session::history`, and use named
operations such as `compact`, `reset`, `replace_provider`, and
`set_reasoning_level`.

`Session::snapshot` is the stable persistence boundary. Serialize it with
`SessionSnapshot::to_json` and restore it with
`SessionOptions::from_snapshot`. SQLite is not required. Snapshots preserve
exact-identity provider replay context but omit raw reasoning and credentials.

## Working directories and capabilities

The SDK never reads the process current directory implicitly. Construct an
absolute, existing `Workspace`, attach it to `RhoBuilder`, and provide an
explicit `WorkspacePolicy`. The default policy denies filesystem, process, and
network operations. Use an `ApprovalHandler` when a permitted operation still
requires a host decision.

Prompt discovery, `AGENTS.md`, skills, environment lookup, logging, terminal
setup, and update checks are host concerns unless an explicit SDK helper is
selected.

## Error handling

Match the non-exhaustive `rho_sdk::Error` classification instead of parsing
application error strings. Provider and tool errors are sanitized. A host may
map typed failures to its own exit codes or protocol without exposing transport
payloads or credentials.
