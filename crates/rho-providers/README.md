# rho-providers

`rho-providers` provides the model provider integrations used by Rho and by
applications built on `rho-sdk`. It includes:

- `build_sdk_provider`, `build_sdk_provider_with_source`, and
  `build_automation_provider` construction helpers
- typed `ProviderBuildOptions`, `ModelError`, and the `CredentialStore` and
  `OsCredentialStore` credential APIs
- the model catalog and `provider_runtime` registry
- provider wire protocols and OAuth login flows
- `set_rho_version`, which lets an embedder identify its application version in
  provider request headers

`build_sdk_provider` opts in to Rho's application credential lookup. It checks
provider-specific environment variables and then the operating system keyring,
so an embedded application can reuse credentials saved by `rho login` without
copying them into another configuration file. Embedders that need different
credential behavior can use `build_sdk_provider_with_source`.

## Providers

The runtime registry includes:

- `openai`
- `openai-codex`
- `anthropic`
- `github-copilot`
- `xai` and `xai-oauth`
- `moonshot`
- `openrouter`
- `kimi-code`

## Usage

This example builds an OpenAI provider from an environment variable or the OS
keyring, attaches it to an SDK runtime, and makes one completion:

```rust,no_run
use rho_providers::build_sdk_provider;
use rho_sdk::{ReasoningLevel, Rho, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = build_sdk_provider("openai", "gpt-5.2", ReasoningLevel::Medium)?;
    let rho = Rho::builder().provider_shared(provider).build()?;
    let session = rho.session(SessionOptions::default()).await?;
    let outcome = session.complete("Explain this repository in one sentence.").await?;
    println!("{}", outcome.text());
    Ok(())
}
```

Call `set_rho_version` before constructing a provider if requests should report
an embedding application's version instead of the crate version.

## Features

The `bundled-sqlite` feature is enabled by default and builds SQLite with the
crate. Disable default features to link `rusqlite` against a compatible system
SQLite installation instead:

```toml
rho-providers = { version = "0.1", default-features = false }
```

## License

Licensed under MIT AND Apache-2.0.
