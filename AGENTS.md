# AGENTS.md

## Conventional commits

Use Conventional Commits for commit messages and PR titles:

```text
<type>(<scope>): <description>
```

- `type` must be one of: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, or `revert`.
- `scope` is optional, but preferred when it makes the affected area clear.
- `description` must be concise, imperative, and lowercase unless it contains a proper noun.
- Do not end the description with a period.
- For breaking changes, add `!` after the type or scope and include a `BREAKING CHANGE:` footer.

Examples:

```text
feat(auth): add token refresh
fix(api): handle empty responses
docs: update setup instructions
chore: bump dependencies
feat(config)!: require explicit config path

BREAKING CHANGE: the default config discovery behavior was removed.
```

## Rust code conventions

- Prefer small, cohesive modules with explicit public API. Keep modules private by default and export only the crate surface that callers actually need.
- Avoid growing already-large files. Add a focused module when new behavior is separable, and keep tests/docs for invariants close to the implementation.
- Keep API call sites self-documenting. Avoid boolean or ambiguous `Option` parameters such as `foo(false)` or `bar(None)`; prefer enums, named methods, builders, or newtypes when practical.
- When a positional literal is unavoidable, add an exact parameter-name comment before opaque literals such as booleans, `None`, and numeric values, for example `set_mode(/*enabled*/ false)`.
- Prefer exhaustive `match` statements over wildcard arms when matching known enums, so future variants force an intentional update.
- Newly added traits should have doc comments explaining their role and expectations for implementors.
- For async traits, avoid `#[async_trait]` and `#[allow(async_fn_in_trait)]`. Prefer returning an explicit future with a `Send` bound, for example `fn run(&self) -> impl std::future::Future<Output = Result<()>> + Send;`.
- Do not create small helper methods that are only used once unless they materially improve readability or isolate a clear invariant.
- Follow common Clippy/rustfmt style: collapse nested `if` statements when possible, inline `format!` arguments (`format!("hello {name}")`), and prefer method references over redundant closures.
- After Rust code changes, run `cargo fmt`. Run the narrowest relevant tests for the changed crate or module before finalizing when practical.

## Abstraction and module boundaries

- Keep generic infrastructure separate from feature-specific policy. Rendering, transport, storage, parsing, and orchestration layers should operate on explicit generic data shapes instead of knowing special cases from individual commands, menus, providers, or features.
- Put feature-specific construction and decisions near the feature that owns them. For example, a picker renderer should understand labels, details, badges, and selection state, while a model picker module decides which model gets a selected badge.
- Prefer explicit interfaces over encoded strings or suffix parsing. If behavior depends on a concept like selected, current, unavailable, warning, or detail text, model it as a field, enum, or small type instead of inferring it from display text.
- When a file starts accumulating unrelated responsibilities, split along ownership boundaries: shared types and mechanics in one module, each feature's setup and policy in its own focused module.
- Design reusable components around stable concepts, not today's specific UI copy or provider names. New features should be able to plug into existing components by providing data, not by adding conditionals to the component.
- Avoid broad abstractions before there are clear boundaries, but once a pattern appears in multiple places, extract the common mechanics and leave the differing policy at the call sites.

## Rust tests

- Prefer integration or behavior-level tests for user-visible logic. Use unit tests for focused pure logic.
- When adding a new test module, prefer a sibling `*_tests.rs` file with an explicit `#[path = "..."] mod tests;` instead of growing implementation files.
- Prefer `pretty_assertions::assert_eq` in tests when available, and compare whole objects rather than asserting field-by-field.
- Do not add tests for statically defined constants or negative tests for behavior that has been removed.
- Avoid mutating process environment in tests; pass environment-derived values or dependencies explicitly where possible.

## rho smoke tests with herdr

- When running inside herdr, you can smoke test rho itself by launching it from source in a sibling pane with `cargo run`.
- Use herdr to control that pane like a user would: split a pane, run `cargo run`, wait for expected output, send text or keys, and read the pane output to verify behavior.
- Prefer this for quick end-to-end checks of terminal UI flows, command handling, startup behavior, and regressions that unit tests do not cover.
- Keep smoke tests focused and lightweight. Capture only relevant excerpts from pane output, and close any temporary panes when they are no longer needed.

## Pull requests

- Prefer the most user-visible Conventional Commit type for the PR title, usually `feat`, `fix`, `docs`, or `refactor`.
- Include a clear summary of what changed and why.
- List tests or validation performed.
- Call out breaking changes with a `BREAKING CHANGE:` section.
