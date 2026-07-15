# SDK compatibility policy

This page defines the in-repository compatibility contract for `rho-sdk` and the
checks used to maintain it. The SDK is currently on the `0.x` development line.
Its stable public contract begins at 1.0.

## Features and capability defaults

`rho-sdk` deliberately declares an empty default feature set:

```toml
rho-sdk = { version = "0.1", default-features = false }
```

The crate currently has no optional Cargo features. As a result, default,
`--no-default-features`, and `--all-features` select the same minimal headless
surface. CI tests all three commands deliberately so a future feature cannot
silently change one mode.

Built-in provider transports, SQLite persistence, operating-system keychain
access, web access, and coding tools are not SDK Cargo features today. They
remain application-owned adapters. If any moves into `rho-sdk`, it must use a
named opt-in feature and must not be added to `default`. Enabling an SDK feature
must not grant workspace capabilities without explicit runtime configuration.

Removing or renaming a public feature, or moving an existing API behind a
feature, is a breaking change. Adding an opt-in feature is additive unless it
changes behavior for existing builds.

## Runtime and supported platforms

The SDK requires a Tokio runtime when it creates sessions or runs. Public
provider and tool futures are `Send`, and the SDK does not initialize a runtime,
terminal, logger, credential store, network client, or global configuration for
the host.

CI compiles and tests the workspace on the current GitHub-hosted stable Rust
toolchain for Linux, macOS, and Windows. The release binary matrix additionally
builds Linux x86_64 GNU, macOS x86_64 and arm64, and Windows x86_64 MSVC
artifacts. Other Rust targets may work, but are not part of the supported CI
platform contract.

## Minimum supported Rust version

The `rho-sdk` minimum supported Rust version (MSRV) is **1.86**. The
`rho-coding-agent` application MSRV is **1.88** because its terminal and
credential dependencies require a newer compiler. Both values are declared as
`package.rust-version` in Cargo metadata.

CI checks `rho-sdk`, including tests and examples, on Rust 1.86 with all
features. It checks the application workspace on Rust 1.88. A dependency update
must preserve these checks or intentionally update this policy and both Cargo
manifests.

An MSRV increase:

- must not be released as a patch version;
- must update Cargo metadata, this page, and the pinned CI toolchain together;
- must be called out in release notes generated for the affected crate; and
- after 1.0, requires at least a minor SDK version increase.

Emergency compiler requirements caused by a security or soundness fix may skip
the normal notice period, but still require all metadata and CI updates in the
same change.

## Semantic versioning and deprecation

Public Rust items, documented event ordering and cancellation behavior, feature
names, and versioned persisted formats are compatibility contracts. CI runs
`cargo-semver-checks` against the pull request base revision whenever that
revision contains `rho-sdk`. The first revision that introduces the crate has no
older SDK baseline; every later pull request has a repository baseline even
before the first crates.io release. Release candidates can also run the workflow
against an explicit revision or tag.

During `0.x`, breaking changes require a minor version bump and must not ship in
a patch. Where practical, an API is deprecated for at least one minor release
before removal. After 1.0, deprecated public APIs remain available until a major
release unless retaining them creates a security or soundness defect.

Every `#[deprecated]` SDK item must provide both `since` and `note`. The note
must identify the replacement or explain why there is none. The compatibility
script enforces this form. Deprecations should include migration documentation
and tests for the replacement API.

## Downstream, conformance, and package checks

The independent workspace in `fixtures/downstream` is intentionally excluded
from the repository workspace. Its crates have exactly one direct dependency:
`rho-sdk`. They compile representative completion, streaming, cancellation,
history, custom-provider, and `Send + Sync` usage without depending on the Rho
application crate or its terminal, configuration, SQLite, or credential code.

CI also performs these checks:

- SDK tests with default, no-default, and all features;
- downstream fixture compilation from its own locked dependency graph;
- workspace tests, Clippy with warnings denied, rustdoc tests, and architecture
  checks;
- `cargo package` verification for `rho-sdk` and package-content validation for
  both crates;
- `cargo publish --dry-run` for `rho-sdk`; and
- a release-mode binary build in each Linux, macOS, and Windows release job
  before that job can upload its asset.

Run the local compatibility checks with:

```sh
python3 scripts/check_sdk_compatibility.py --test-features --test-downstream
```

Package checks validate repository artifacts only. Actual registry publication,
registry indexing, and external integration feedback remain separate release
steps and are not implied by a passing CI run.
