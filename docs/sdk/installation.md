# SDK installation and support

## Add the dependency

`rho-sdk` has not reached its stable 1.0 contract. Until a registry release is announced, use a repository checkout, a workspace path, or a pinned Git revision rather than assuming a crates.io version exists.

From a workspace checkout:

```toml
[dependencies]
rho-sdk = { path = "crates/rho-sdk", default-features = false }
```

From Git, pin an exact reviewed revision for reproducible builds:

```toml
[dependencies]
rho-sdk = { git = "https://github.com/matthewyjiang/rho", rev = "<full-commit-hash>", package = "rho-sdk", default-features = false }
```

After a registry release is published, use the released version and a lockfile. The coordinated publication procedure is described in [release candidates](/sdk/release-candidates).

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

A numeric minimum supported Rust version, or MSRV, is **not currently declared** in `Cargo.toml`. CI uses the rolling stable Rust toolchain. Therefore the current pre-1.0 support statement is "current stable Rust", not a fixed older compiler.

Before `1.0.0-rc.1`, maintainers must:

1. select and document a numeric MSRV
2. declare the same value with Cargo's `rust-version` field
3. build and test the SDK with that toolchain
4. verify that enabled dependencies support it
5. apply the [deprecation and compatibility policy](/sdk/compatibility#rust-version-policy)

A release candidate must not claim a fixed MSRV until those checks pass. After 1.0, an MSRV increase must be called out in release notes and follow the documented compatibility policy.

## Runtime and dependency expectations

- The SDK is a library and does not create `~/.rho` files.
- The SDK does not read credentials or environment variables unless a host-provided adapter does so.
- The SDK does not provide an implicit global singleton. A host builds a `Rho` runtime and owns its lifetime.
- A host should call `Rho::shutdown` for coordinated teardown. See [shutdown semantics](/sdk/events-and-cancellation#shutdown-contract).
- The host owns provider transport setup, network policy, secrets, persistence location, and logging.
