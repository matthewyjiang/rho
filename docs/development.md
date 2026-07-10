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

## Architecture guardrails

Run the lightweight architecture checks before submitting structural changes:

```bash
python3 scripts/check_architecture.py
python3 scripts/check_architecture.py --self-test
```

The script uses only the Python standard library and enforces these repository policies:

- Hand-written production Rust files under `src/`, plus `build.rs`, have a 1,000-line default budget.
- Dedicated test files, including files under a `tests/` directory and `*_test.rs`, `*_tests.rs`, or `tests.rs`, are excluded. Inline tests still count toward their production file's budget.
- Generated Rust files are excluded only when their exact path and reason are recorded in `GENERATED_RUST_FILES` in the script. There are currently no generated-file exclusions.
- Existing oversized production files are listed explicitly in `LEGACY_FILE_LINE_BUDGETS`. Their ceilings prevent further growth and should be lowered or removed as the files are split.
- `src/credentials.rs` must remain independent of `crate::model`, keeping credential storage separate from model runtime metadata.
- `src/main.rs` has a 50-line thin-binary budget so application orchestration remains in the library.

Current legacy file budgets are:

| File | Maximum lines |
| --- | ---: |
| `src/tui.rs` | 7,753 |
| `src/model/openai/codex_ws.rs` | 1,036 |

Do not raise a budget just to make a check pass. Prefer extracting a cohesive module and reducing the recorded ceiling. If a generated file must be added, list its exact repository-relative path with a non-empty reason so the exclusion remains reviewable. When changing the scanner or policy, update its self-tests and this documentation together.
