# SDK installation and support

## Add the dependency

`rho-sdk` is published on [crates.io](https://crates.io/crates/rho-sdk). Add it to your crate and commit a lockfile. Prefer a major-compatible Cargo range so you receive compatible minors automatically:

```toml
[dependencies]
rho-sdk = { version = "1", default-features = false }
```

Tighten the range only when you need a specific build for regression hunting. For release-to-release changes, see the [SDK changelog](/sdk/changelog).

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

Historical process notes for the original stable cutover live under [release candidates](/sdk/release-candidates).

## Async runtime

The SDK requires a [Tokio](https://tokio.rs/) runtime. Session runs spawn Tokio tasks and use Tokio channels and synchronization. Call SDK async entrypoints from a Tokio runtime. Provider, tool, compactor, approval, hook, and usage-recorder extension points return explicit futures with a `Send` bound, and their traits are `Send + Sync`.

The runtime is headless. It does not initialize terminal state, a global logger, an update checker, or an async runtime on the host's behalf.

## Cargo features

The `rho-sdk` manifest has an empty default feature set and defines no named optional features.

| Invocation | Current result |
| --- | --- |
| `cargo check -p rho-sdk` | Core SDK only |
| `cargo check -p rho-sdk --no-default-features` | Same core SDK surface |
| `cargo check -p rho-sdk --all-features` | Same core SDK surface while no named features exist |

Built-in production providers, SQLite, keychain access, web access, and coding tools are not silently included. If any of those integrations move into the SDK, they must be introduced as explicit adapters or deliberately named opt-in features, documented here, and tested in supported combinations. The application crate's `bundled-sqlite` feature is not an SDK feature.

## Platform support and validation

Desktop targets are Linux, macOS, and Windows. Current repository CI performs:

- the complete workspace tests, Clippy, formatting, packaging checks, and feature checks on `ubuntu-latest`
- workspace compile checks on `macos-latest` and `windows-latest`
- focused Bash behavior tests on macOS

This is the current validation matrix, not a claim that every provider, host tool, credential adapter, or operating-system integration has been exercised on all three systems. Hosts must test their own adapters on every platform they support. No mobile, WebAssembly, or non-Rust binding is part of the supported target set.

## Minimum supported Rust version

MSRV is a compatibility contract. Published values live in
[compatibility](/sdk/compatibility#minimum-supported-rust-version) and must match
each crate's `package.rust-version` field. CI fails if the policy and Cargo
metadata disagree.

An MSRV increase must be called out in release notes and follow the
[deprecation and compatibility policy](/sdk/compatibility#minimum-supported-rust-version).

## Runtime and dependency expectations

- The SDK is a library and does not create `~/.rho` files.
- The SDK does not read credentials or environment variables unless a host-provided adapter does so.
- The SDK does not provide an implicit global singleton. A host builds a `Rho` runtime and owns its lifetime.
- A host should call `Rho::shutdown` for coordinated teardown. See [shutdown semantics](/sdk/events-and-cancellation#shutdown-contract).
- The host owns provider transport setup, network policy, secrets, persistence location, logging, hook programs, and usage sinks.
