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

`build_sdk_provider` takes an explicit credential source. It does not pick an OS
or file store on its own. Embedders typically wrap `OsCredentialStore` (or
another `CredentialStore`) in an application credential adapter and pass that
source in. Prefer `build_sdk_provider_with_source` when you already have
`ProviderBuildOptions`.

Credential backends are `os` (default) and `file`. Parsing accepts `"auto"` as an
alias for `os` only; there is never a silent fallback to file storage.

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

This example builds an OpenAI provider from an explicit OS credential store,
attaches it to an SDK runtime, and makes one completion:

```rust,no_run
use std::sync::Arc;

use rho_providers::{
    auth::provider_credentials::ApplicationCredentialSource, build_sdk_provider,
    OsCredentialStore,
};
use rho_sdk::{ReasoningLevel, Rho, SessionOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let credentials = ApplicationCredentialSource::new(Arc::new(OsCredentialStore));
    let provider =
        build_sdk_provider("openai", "gpt-5.2", ReasoningLevel::Medium, &credentials)?;
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
