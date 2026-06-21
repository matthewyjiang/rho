# Plan 001: Use a static model list for Codex auth

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving to the next step. If anything in the "STOP conditions" section occurs, stop and report. Do not improvise. When done, update the status row for this plan in `plans/README.md` unless a reviewer told you they maintain the index.
>
> **Drift check (run first)**: `git diff --stat 6b9393d..HEAD -- src/model/openai/mod.rs src/tui.rs docs/interactive-tui.md docs/guide/index.md`
> If any in-scope file changed since this plan was written, compare the "Current state" excerpts against the live code before proceeding. On a mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `6b9393d`, 2026-06-21

## Why this matters

Rho currently errors when the user runs `/model` while using Codex auth because Codex auth cannot list models from the provider. This makes the model picker unusable even though model selection can work with an explicit model name. Pi handles this class of issue by keeping a curated built-in list of tool-capable models, so Rho should do the same for Codex auth and reserve dynamic provider listing for API-key auth.

## Current state

Relevant files:

- `src/model/openai/mod.rs` contains `OpenAiProvider`, auth selection, dynamic OpenAI API-key model listing, and Codex request handling.
- `src/tui.rs` calls `agent.provider_mut().list_models().await` when `/model` has no argument and shows any returned models in a picker.
- `docs/interactive-tui.md` describes `/model [model]` as a provider-backed picker.
- `docs/guide/index.md` documents API-key auth and Codex auth examples.

Current code excerpts:

```rust
// src/model/openai/mod.rs:67-73
pub async fn list_models(&self) -> Result<Vec<String>, ModelError> {
    match &self.auth {
        Auth::ApiKey(key) => self.list_openai_models(key).await,
        Auth::Codex { .. } => Err(ModelError::InvalidResponse(
            "model listing is not available for Codex auth yet".into(),
        )),
    }
}
```

```rust
// src/model/openai/mod.rs:389-407
impl OpenAiProvider {
    async fn list_openai_models(&self, key: &str) -> Result<Vec<String>, ModelError> {
        let url = format!("{}/models", self.api_base.trim_end_matches('/'));
        let response: ModelsResponse = self
            .client
            .get(url)
            .bearer_auth(key)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut models = response
            .data
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();
        models.sort();
        Ok(models)
    }
```

```rust
// src/tui.rs:833-843
self.status = "loading models".into();
terminal.draw(|frame| self.draw(frame))?;
let models = match agent.provider_mut().list_models().await {
    Ok(models) => models,
    Err(err) => {
        self.insert_entry(
            terminal,
            &Entry::Error(format!("could not load models from provider: {err}")),
        )?;
        self.status = "model list failed".into();
        return Ok(());
    }
};
```

```markdown
<!-- docs/interactive-tui.md:68-70 -->
| Command | Action |
| --- | --- |
| `/model [model]` | Open a provider-backed model picker, or switch directly to `model` and save it to config. |
```

Repo conventions to follow:

- Rust 2021, `anyhow` at app boundaries, `thiserror` for model errors.
- Unit tests live inline under `#[cfg(test)] mod tests` in the same source file. `src/model/openai/mod.rs:1123` already has provider-focused tests.
- CI requires `cargo fmt --all -- --check`, `cargo test --all`, and `cargo clippy --all-targets --all-features -- -D warnings`.
- Commit and PR titles use Conventional Commits. Example from git history: `feat(session): persist interactive sessions`.

## Commands you will need

| Purpose | Command | Expected on success |
| --- | --- | --- |
| Format | `cargo fmt --all -- --check` | exit 0, no formatting diff |
| Tests | `cargo test --all` | exit 0, all tests pass |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0, no warnings |
| Docs build | `npm run docs:build` | exit 0, VitePress build succeeds |

## Scope

**In scope**:

- `src/model/openai/mod.rs`
- `docs/interactive-tui.md`
- `docs/guide/index.md`
- `plans/README.md` status update only

**Out of scope**:

- `src/tui.rs`, unless the existing picker code cannot consume the new provider behavior. The preferred fix is in the provider because `open_model_picker` already handles any successful list.
- Auth refresh, Codex login, API base configuration, or adding new providers.
- Changing the default model in `src/config.rs`. That is a product decision separate from this bug.
- Making API-key auth use a static list. API-key auth should keep using `/models` for now.

## Git workflow

- Suggested branch: `advisor/001-static-codex-models`.
- Commit message: `fix(model): use static list for Codex models`.
- Keep the change focused. Do not push or open a PR unless instructed.

## Steps

### Step 1: Add a curated Codex model list helper

In `src/model/openai/mod.rs`, add a private constant and helper near the `Auth` enum or the first `impl OpenAiProvider` block.

Target behavior:

- Codex auth returns a local list without making any HTTP request.
- The list is sorted and deduplicated before display.
- The provider's current configured model is included even if it is not in the curated list. This preserves custom or newly released model names that users already configured with `--model` or `/model <name>`.
- The first curated list must include at least `gpt-5.5`, because `src/config.rs` defaults to it and `docs/guide/index.md` uses it in the Codex example. Add any other Codex models the maintainer is confident are supported by the current Codex responses endpoint, but do not guess a long list.

Suggested shape:

```rust
const CODEX_MODELS: &[&str] = &[
    "gpt-5.5",
];

fn codex_models(current_model: &str) -> Vec<String> {
    let mut models = CODEX_MODELS
        .iter()
        .copied()
        .chain(std::iter::once(current_model))
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}
```

If the maintainer knows more supported Codex model IDs, add them to `CODEX_MODELS` in the same constant. Keep the helper private.

**Verify**: `cargo fmt --all -- --check` should exit 0 after formatting is applied in the next step. It may fail now if formatting changed; that is acceptable until Step 3.

### Step 2: Return the static list for Codex auth

Change `OpenAiProvider::list_models` so the Codex branch returns the helper result instead of `ModelError::InvalidResponse`.

Target shape:

```rust
pub async fn list_models(&self) -> Result<Vec<String>, ModelError> {
    match &self.auth {
        Auth::ApiKey(key) => self.list_openai_models(key).await,
        Auth::Codex { .. } => Ok(codex_models(&self.model)),
    }
}
```

Do not alter `list_openai_models`; API-key auth should still call the OpenAI `/models` endpoint.

**Verify**: `cargo test --all model_getter_and_setter_update_provider_model` should exit 0.

### Step 3: Add provider tests for Codex listing

In `src/model/openai/mod.rs` under the existing `#[cfg(test)] mod tests`, add tests that construct `OpenAiProvider` directly with `Auth::Codex` and dummy tokens. Do not call `OpenAiProvider::new_with_reasoning` in these tests, because that would require real Codex credentials.

Add at least these tests:

1. `codex_list_models_returns_static_models`
   - Build an `OpenAiProvider` with `auth: Auth::Codex { access_token: "token".into(), refresh_token: None, account_id: None, auth_path: None }` and `model: "gpt-5.5".into()`.
   - Call `provider.list_models().await.unwrap()`.
   - Assert the returned list contains `"gpt-5.5"`.
   - Assert the list is non-empty.

2. `codex_list_models_includes_current_custom_model`
   - Build the same provider with `model: "custom-codex-model".into()`.
   - Assert the returned list contains both `"custom-codex-model"` and `"gpt-5.5"`.
   - Assert the list is sorted by comparing it to a sorted clone.

Use `#[tokio::test]` for async tests. Keep names lowercase with underscores like the existing tests.

**Verify**: `cargo test --all codex_list_models` should exit 0 and run the new tests.

### Step 4: Update documentation to describe built-in Codex models

Update docs so users understand the behavior:

- In `docs/interactive-tui.md`, change the `/model [model]` row from "provider-backed model picker" to wording that covers both API-backed and built-in lists. Example: "Open a model picker using the provider's API when available or Rho's built-in list for Codex auth, or switch directly to `model` and save it to config."
- In `docs/guide/index.md` under "Codex OAuth", add one sentence after the example: "With Codex auth, `/model` uses Rho's built-in Codex model list because Codex auth does not expose provider model listing. You can still run `rho --auth codex --model <model>` or `/model <model>` to use a specific model."

Do not document unsupported model IDs beyond the examples already present unless they are in `CODEX_MODELS`.

**Verify**: `npm run docs:build` should exit 0.

### Step 5: Run the full verification gate

Run the same checks CI runs, plus docs because this plan changes docs:

1. `cargo fmt --all -- --check`
2. `cargo test --all`
3. `cargo clippy --all-targets --all-features -- -D warnings`
4. `npm run docs:build`

Expected result: all commands exit 0.

## Test plan

- New tests in `src/model/openai/mod.rs` cover Codex auth model listing without real credentials or network access.
- Existing API-key behavior is protected by leaving `list_openai_models` untouched and by running the full test suite.
- Documentation build confirms the markdown changes do not break VitePress.

## Done criteria

All must hold:

- [ ] `/model` with Codex auth no longer emits `could not load models from provider: provider returned invalid response: model listing is not available for Codex auth yet`.
- [ ] `OpenAiProvider::list_models` returns `Ok(Vec<String>)` for `Auth::Codex`.
- [ ] Codex model list tests exist and pass without real credentials.
- [ ] API-key auth still calls `list_openai_models`.
- [ ] `cargo fmt --all -- --check` exits 0.
- [ ] `cargo test --all` exits 0.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `npm run docs:build` exits 0.
- [ ] No files outside the in-scope list are modified, except build artifacts already ignored by the repo.
- [ ] `plans/README.md` status row for this plan is updated.

## STOP conditions

Stop and report back if:

- The code at `src/model/openai/mod.rs:67-73` no longer resembles the excerpt in this plan.
- `OpenAiProvider::list_models` has been generalized to multiple provider types before you start, because this plan assumes the current OpenAI-only provider shape.
- The maintainer wants a larger Codex model catalog but cannot identify exact supported model IDs. Do not invent model names.
- Any verification command fails twice after reasonable fixes.
- Fixing the issue appears to require changing auth refresh, Codex login, or TUI picker architecture.

## Maintenance notes

- The curated `CODEX_MODELS` list will need updates as Codex-supported models change. Keep the list small and evidence-based.
- Including the current configured model is intentional. It lets users continue using a model before Rho's curated list has been updated.
- Reviewers should verify that Codex listing does not make a network request and that API-key listing behavior did not change.
