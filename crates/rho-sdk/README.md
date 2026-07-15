# rho-sdk

`rho-sdk` is the embeddable, headless agent runtime used by the Rho coding agent.
The crate is under active development toward its first stable release.

## Minimal completion

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

## Runtime behavior

A session permits one active run. `Session::complete` and `Session::start` use
the same provider and tool loop. Streaming runs use bounded channels for
backpressure, produce one terminal event, and expose their final typed outcome
without requiring hosts to reconstruct it from deltas. Dropping a run cancels
and aborts provider or tool work.

The SDK currently requires a Tokio runtime. Provider and tool extension points
return explicit `Send` futures and may be used as trait objects.

## Security defaults

The default feature set is empty. Creating an SDK runtime will not implicitly
read environment variables, access an operating-system credential store, write
to `~/.rho`, initialize a terminal or logger, check for updates, or grant tools
filesystem, process, or network access.

The crate currently has no optional Cargo features and its default feature set
is empty. Provider transports, persistence adapters, keychain access, web
access, and built-in coding tools will each be opt-in as they are extracted.

Capabilities such as built-in providers, SQLite persistence, keychain access,
web access, and coding tools will be introduced behind explicit adapters and
opt-in Cargo features as their public contracts are stabilized.

See [the Rho repository](https://github.com/matthewyjiang/rho) and
[the SDK tracking issue](https://github.com/matthewyjiang/rho/issues/256) for
the current roadmap.
