# SDK installation and support

## Add the dependency

`rho-sdk 1.0.0` is published on [crates.io](https://crates.io/crates/rho-sdk). Depend on the released version and commit a lockfile:

```toml
[dependencies]
rho-sdk = { version = "1.0", default-features = false }
```

From a workspace checkout, for example when developing alongside this repository:

```toml
[dependencies]
rho-sdk = { path = "crates/rho-sdk", default-features = false }
```

From Git, pin an exact reviewed revision for reproducible builds against unreleased changes:

```toml
[dependencies]
rho-sdk = { git = "https://github.com/matthewyjiang/rho", rev = "<full-commit-hash>", package = "rho-sdk", default-features = false }
```

The coordinated publication procedure for future major/minor releases is described in [release candidates](/sdk/release-candidates).

## Async runtime

The current SDK requires a [Tokio](https://tokio.rs/) runtime. Session runs spawn Tokio tasks and use Tokio channels and synchronization. Call SDK async entrypoints from a Tokio runtime. Provider, tool, compactor, and approval extension points return explicit futures with a `Send` bound, and their traits are `Send + Sync`.

The runtime is headless. It does not initialize terminal state, a global logger, an update checker, or an async runtime on the host's behalf.

## Cargo features

The current `rho-sdk` manifest has an empty default feature set and defines no named optional features.

| Invocation | Current result |
| --- | --- |
| `cargo check -p rho-sdk` | Core SDK only |
| `cargo check -p rho-sdk --no-default-features` | Same core SDK surface |
| `cargo check -p rho-sdk --all-features` | Same core SDK surface while no named features exist |

Built-in production providers, SQLite, keychain access, web access, and coding tools are not silently included. If any of those integrations move into the SDK, they must be introduced as explicit adapters or deliberately named opt-in features, documented here, and tested in supported combinations. The application crate's `bundled-sqlite` feature is not an SDK feature.

## Platform support and validation

The intended 1.0 desktop targets are Linux, macOS, and Windows. Current repository CI performs:

- the complete workspace tests, Clippy, formatting, packaging checks, and feature checks on `ubuntu-latest`
- workspace compile checks on `macos-latest` and `windows-latest`
- focused Bash behavior tests on macOS

This is the current validation matrix, not a claim that every provider, host tool, credential adapter, or operating-system integration has been exercised on all three systems. Hosts must test their own adapters on every platform they support. No mobile, WebAssembly, or non-Rust binding is part of the 1.0 target.

## Minimum supported Rust version

The `rho-sdk` minimum supported Rust version (MSRV) is Rust 1.86. The application requires Rust 1.92 because of its terminal, credential, and terminal-native Mermaid rendering dependencies. Each crate declares its MSRV with Cargo's `package.rust-version` field, and CI builds the crate with that exact compiler version in addition to testing on current stable Rust.

The Cargo manifests are the source of truth for these versions. CI reads them directly rather than maintaining a separate copy of each version. An MSRV increase must be called out in release notes and follow the [deprecation and compatibility policy](/sdk/compatibility#minimum-supported-rust-version).

## Runtime and dependency expectations

- The SDK is a library and does not create `~/.rho` files.
- The SDK does not read credentials or environment variables unless a host-provided adapter does so.
- The SDK does not provide an implicit global singleton. A host builds a `Rho` runtime and owns its lifetime.
- A host should call `Rho::shutdown` for coordinated teardown. See [shutdown semantics](/sdk/events-and-cancellation#shutdown-contract).
- The host owns provider transport setup, network policy, secrets, persistence location, and logging.
