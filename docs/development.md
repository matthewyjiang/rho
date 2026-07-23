# Development

Build and check the project with Cargo:

```bash
cargo build
cargo test
```

Run the local binary without installing:

```bash
cargo run --
cargo run -- run "summarize this repository"
```

Use the local binary to test the [interactive TUI](/interactive-tui), [automation mode](/automation-cli), [configuration](/configuration), and [tools](/tools-workspace) behavior while developing.

## Interactive TUI PTY harness

Rho includes a deterministic PTY harness in `crates/rho-tui-pty` for automated interactive TUI tests. Prefer it over manual Herdr smoke tests for regressions that can be expressed as scripted scenarios.

### Layers

- **PTY controller** - spawn a selected `rho` binary in a pseudo-terminal, inject keys/paste/mouse, resize, drain output, and kill-on-drop
- **Screen model** - reconstruct the visible terminal with a VT parser and assert user-visible text
- **Scenarios** - named action/assertion sequences over `RHO_TUI_TEST_MODE=matrix`
- **Artifacts** - on failure, keep raw PTY bytes, reconstructed screen, action log, and redacted env

Unix PTYs are supported. Windows is skipped with an explicit error rather than a silent pass.

### Run harness self-tests

```bash
cargo test -p rho-tui-pty
```

### Run the CI smoke scenarios

```bash
cargo test -p rho-coding-agent --test tui_pty
```

Smoke scenarios cover startup/stream/exit, cancel-and-resubmit, resize-during-stream, scroll-during-stream, and terminal restoration.

### Run one scenario locally

```bash
cargo build -p rho-coding-agent
cargo run -p rho-tui-pty --bin rho-pty-scenario -- --list
cargo run -p rho-tui-pty --bin rho-pty-scenario -- --bin target/debug/rho startup_stream_exit
cargo run -p rho-tui-pty --bin rho-pty-scenario -- --bin target/debug/rho --smoke
cargo run -p rho-tui-pty --bin rho-pty-scenario -- --bin target/debug/rho --timing startup_stream_exit
```

Failure artifacts default to a temp directory (or `--artifacts <dir>`). Successful runs do not retain artifacts.

### Environment isolation

Scenarios launch Rho with:

- temporary `HOME` and `--config`
- `RHO_TUI_TEST_MODE=matrix` (debug builds only)
- host terminal markers stripped (`TMUX`, `TERM_PROGRAM`, Herdr vars, editor markers, and related identity env)
- `check_for_updates = false` and `web_search_provider = "disabled"` in the isolated config

### When to use Herdr instead

Use the Herdr sibling-pane workflow for exploratory checks, novel bugs that are not yet encoded as scenarios, or parity checks against a real terminal renderer. See the `rho-tui-pty-testing` and `rho-tui-herdr-testing` skills.

## Model integration layers

Model integrations are split into three layers:

- `crates/rho-providers/src/model/` defines provider registry, catalog, and application model support without wire types.
- `crates/rho-providers/src/protocol/` converts the canonical SDK model to and from API wire formats. OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, and Google Gemini Generate Content are implemented here.
- `crates/rho-providers/src/providers/` owns credentials, endpoint selection, headers, retries, continuation state, and transport policy for each provider. Multiple providers may consume one protocol codec.

Keep provider-specific fields in protocol or provider modules unless the agent needs the underlying concept. Adding a protocol stub does not make a provider available: provider identity, authentication, model discovery, runtime construction, and documentation must be implemented separately.

## Architecture guardrails

Run the lightweight architecture checks before submitting structural changes:

```bash
python3 scripts/check_architecture.py
python3 scripts/check_architecture.py --self-test
```

The script uses only the Python standard library and reads policy from `scripts/architecture.json`. It enforces these repository policies:

- Hand-written production Rust files under workspace crate `src/` directories, plus crate `build.rs` files, have a 1,000-line default budget (`default_production_line_budget`).
- Dedicated test files, including files under a `tests/` directory and `*_test.rs`, `*_tests.rs`, or `tests.rs`, are excluded. Inline tests still count toward their production file's budget.
- Generated Rust files are excluded only when their exact path and reason are recorded in `generated_files` in `scripts/architecture.json`. There are currently no generated-file exclusions.
- Existing oversized production files are listed explicitly in `legacy_file_budgets`. Their ceilings prevent further growth and should be lowered or removed as the files are split.
- `crates/rho-providers/src/credentials` must remain independent of `model`, keeping credential storage separate from model runtime metadata (`forbidden_dependencies`).
- `crates/rho/src/main.rs` has a 50-line thin-binary budget so application orchestration remains in the library (`thin_binary_budgets`).
- Package dependency boundaries are also declared in `forbidden_package_dependencies` so lower-level crates cannot depend on the application or reverse the intended layering.

Current legacy file budgets are:

| File | Maximum lines |
| --- | ---: |
| `crates/rho/src/tui.rs` | 5,050 |

Do not raise a budget just to make a check pass. Prefer extracting a cohesive module and reducing the recorded ceiling. If a generated file must be added, list its exact repository-relative path with a non-empty reason so the exclusion remains reviewable. When changing the scanner or policy, update its self-tests and this documentation together.
