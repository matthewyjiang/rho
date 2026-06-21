# Plan 002: Add a static cross-provider model catalog

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving to the next step. If anything in the "STOP conditions" section occurs, stop and report. Do not improvise. When done, update the status row for this plan in `plans/README.md` unless a reviewer told you they maintain the index.
>
> **Drift check (run first)**: `git diff --stat 6b9393d..HEAD -- src/agent.rs src/config.rs src/main.rs src/model/mod.rs src/model/openai/mod.rs src/tui.rs docs/interactive-tui.md docs/guide/index.md plans/001-use-static-codex-model-list.md`
> If any in-scope file changed since this plan was written, compare the "Current state" excerpts against the live code before proceeding. On a mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: feature
- **Planned at**: commit `6b9393d`, 2026-06-21

## Why this matters

The earlier narrow plan only fixes Codex by returning a hardcoded OpenAI/Codex list from `OpenAiProvider`. The desired behavior is broader: Rho should have one static catalog of known models across providers, `/model` should show all models from all available providers, and choosing an item should update both `provider` and `model`. This avoids provider-side model listing failures and establishes the architecture needed for future providers.

## Current state

Relevant files:

- `src/config.rs` stores `provider` and `model` separately, defaulting to `openai` and `gpt-5.5`.
- `src/main.rs` rejects any provider other than `openai`, builds an `OpenAiProvider`, and constructs `Agent<OpenAiProvider>`.
- `src/agent.rs` stores the provider as a generic type `P: ModelProvider` and only exposes `provider_mut()`.
- `src/model/openai/mod.rs` has inherent `model`, `set_model`, and `list_models` methods on `OpenAiProvider`; Codex currently returns an invalid-response error for listing.
- `src/tui.rs` is typed to `Agent<OpenAiProvider>`, uses `OpenAiProvider::list_models()` for `/model`, and `select_model` only saves `config.model`.

Current code excerpts:

```rust
// src/config.rs:5-12
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub provider: String,
    pub model: String,
    pub max_output_bytes: usize,
    pub auth: String,
    pub reasoning_effort: String,
    pub reasoning_summary: String,
}
```

```rust
// src/main.rs:48-74
if cfg.provider != "openai" {
    anyhow::bail!(
        "unsupported provider '{}': only 'openai' is implemented",
        cfg.provider
    );
}
...
let provider = OpenAiProvider::new_with_reasoning(
    cfg.model.clone(),
    auth_mode,
    reasoning_config_value(&cfg.reasoning_effort),
    reasoning_config_value(&cfg.reasoning_summary),
)?;
```

```rust
// src/agent.rs:36-44
pub struct Agent<P: ModelProvider> {
    provider: P,
    tools: ToolRegistry,
    ctx: ToolContext,
    messages: Vec<Message>,
    message_sink: Option<MessageSink>,
}

impl<P: ModelProvider> Agent<P> {
```

```rust
// src/tui.rs:834-852
async fn open_model_picker(
    &mut self,
    terminal: &mut DefaultTerminal,
    agent: &mut Agent<OpenAiProvider>,
) -> anyhow::Result<()> {
    self.status = "loading models".into();
    terminal.draw(|frame| self.draw(frame))?;
    let models = match agent.provider_mut().list_models().await {
        Ok(models) => models,
        Err(err) => {
            self.insert_entry(
                terminal,
                &Entry::Error(format!("could not load models from provider: {err}")),
            )?;
```

```rust
// src/tui.rs:915-925
fn select_model(
    &mut self,
    model: String,
    terminal: &mut DefaultTerminal,
    agent: &mut Agent<OpenAiProvider>,
) -> anyhow::Result<()> {
    agent.provider_mut().set_model(model.clone());
    self.info.model = model.clone();
    match Config::load(self.info.config_path.clone()).and_then(|mut config| {
        config.model = model.clone();
        config.save(self.info.config_path.clone())
    }) {
```

Repo conventions to follow:

- Rust 2021, `anyhow` at application boundaries, `thiserror` for typed errors.
- Unit tests live inline under `#[cfg(test)] mod tests` in the same source file.
- CI requires `cargo fmt --all -- --check`, `cargo test --all`, and `cargo clippy --all-targets --all-features -- -D warnings`.
- Commit and PR titles use Conventional Commits. Suggested commit: `feat(model): add static model catalog`.

## Commands you will need

| Purpose | Command | Expected on success |
| --- | --- | --- |
| Format | `cargo fmt --all -- --check` | exit 0, no formatting diff |
| Tests | `cargo test --all` | exit 0, all tests pass |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0, no warnings |
| Docs build | `npm run docs:build` | exit 0, VitePress build succeeds |

## Scope

**In scope**:

- `src/model/mod.rs`
- `src/model/openai/mod.rs`
- new file `src/model/catalog.rs`
- new file `src/model/provider.rs` or equivalent provider factory/router module
- `src/agent.rs`
- `src/main.rs`
- `src/tui.rs`
- `docs/interactive-tui.md`
- `docs/guide/index.md`
- `plans/README.md` status update only

**Out of scope**:

- Implementing new provider APIs beyond OpenAI/Codex. The catalog may contain future provider/model entries, but non-implemented providers must not be selectable as available.
- OAuth/login flows.
- Dynamic provider model listing from remote `/models` endpoints. This feature should stop depending on remote listing for `/model`.
- Changing tool schemas or agent message persistence.
- Merging or executing `plans/001-use-static-codex-model-list.md`; this plan supersedes it.

## Git workflow

- Suggested branch: `advisor/002-static-model-catalog`.
- Suggested commit: `feat(model): add static model catalog`.
- Keep changes focused. Do not push or open a PR unless instructed.

## Design requirements

Implement these semantics:

1. Rho owns a static catalog of known models. The catalog entries contain at least:
   - `provider`: stable provider id such as `openai`.
   - `model`: exact model id sent to the provider.
   - `display_name`: human-friendly label, optional if identical to `model`.
   - `auth_modes`: which auth modes are allowed for this provider/model, e.g. `api-key`, `codex`, or both.
   - `available`: derived from whether Rho has an implementation for the provider. Do not expose future-provider entries until their provider implementation exists.
2. `/model` with no args shows all available catalog entries across all implemented providers, not just the current provider.
3. Picker rows show provider and model, for example `openai / gpt-5.5`; the current provider+model row is marked `current`.
4. Selecting a row switches both provider and model, updates `self.info.provider`, `self.info.model`, the live agent provider, and persists both `config.provider` and `config.model`.
5. `/model <arg>` switches both provider and model too:
   - Accept `provider/model` as the canonical explicit syntax, e.g. `/model openai/gpt-5.5`.
   - Accept a bare model id only if it uniquely matches one available catalog entry. If ambiguous, show an error telling the user to use `provider/model`.
   - If the bare model is not in the catalog, preserve today's escape hatch for OpenAI by treating it as the current provider plus that model. This allows newly released models before the catalog is updated.
6. Codex auth must never call a remote model-list endpoint. It uses the same static catalog as every other auth mode.

## Steps

### Step 1: Add a static model catalog module

Create `src/model/catalog.rs` and export it from `src/model/mod.rs` with `pub mod catalog;`.

Suggested public API:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelCatalogEntry {
    pub provider: &'static str,
    pub model: &'static str,
    pub display_name: &'static str,
    pub auth_modes: &'static [&'static str],
}

pub const MODEL_CATALOG: &[ModelCatalogEntry] = &[
    ModelCatalogEntry {
        provider: "openai",
        model: "gpt-5.5",
        display_name: "gpt-5.5",
        auth_modes: &["api-key", "codex"],
    },
];
```

Add helper functions:

- `implemented_providers() -> &'static [&'static str]`, initially `&["openai"]`.
- `available_models(auth: &str) -> Vec<ModelCatalogEntry>`: filters catalog entries to implemented providers and entries whose `auth_modes` contains `auth`; sort by provider then model.
- `resolve_model_selection(input: &str, current_provider: &str, auth: &str) -> Result<ModelSelection, ModelSelectionError>`.
- `ModelSelection { provider: String, model: String, from_catalog: bool }`.

`resolve_model_selection` should implement the direct command behavior described in Design requirement 5. Put its unit tests in this module.

Initial catalog entries should include `openai/gpt-5.5`, because that is the current default and docs example. If maintainers know additional exact model IDs supported by both API key and Codex, add them. Do not guess IDs.

**Verify**: `cargo test --all catalog` should run the new catalog tests and exit 0.

### Step 2: Stop using remote model listing for the TUI picker

In `src/tui.rs`, replace the call to `agent.provider_mut().list_models().await` in `open_model_picker` with `catalog::available_models(&self.info.auth_or_config_auth)` equivalent.

Because `TuiInfo` currently does not carry auth, add `pub auth: String` to `TuiInfo` in `src/tui.rs` and populate it from `cfg.auth` in `src/main.rs`.

Update picker items so `PickerItem.value` stores the canonical `provider/model` string and `PickerItem.label` shows the same canonical string or a friendly display like `openai / gpt-5.5`. The description should be `current` only when both provider and model match `self.info.provider` and `self.info.model`.

Do not call `OpenAiProvider::list_models` from `/model` anymore.

**Verify**: `cargo test --all` should still compile. If it fails because `TuiInfo` construction in tests or main is missing `auth`, update those call sites only.

### Step 3: Add provider replacement support to Agent

Add a method to `Agent<P>` in `src/agent.rs`:

```rust
pub fn replace_provider(&mut self, provider: P) {
    self.provider = provider;
}
```

This lets the TUI replace the live provider after `/model` changes provider/model. Add a small unit test if there are existing agent tests nearby; otherwise rely on the TUI/main compile path.

**Verify**: `cargo test --all agent` should exit 0, or `cargo test --all` if there is no agent-specific test target.

### Step 4: Introduce a provider factory/router for current and future providers

Create a model provider construction function so both `main.rs` startup and `tui.rs` model switching use the same logic.

Suggested location: `src/model/provider.rs`, exported from `src/model/mod.rs`.

Suggested API:

```rust
pub fn build_provider(
    provider: &str,
    model: &str,
    auth: &str,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
) -> anyhow::Result<OpenAiProvider> {
    match provider {
        "openai" => {
            let auth_mode = match auth {
                "codex" => AuthMode::Codex,
                _ => AuthMode::ApiKey,
            };
            OpenAiProvider::new_with_reasoning(
                model.to_string(),
                auth_mode,
                reasoning_effort,
                reasoning_summary,
            )
            .map_err(Into::into)
        }
        other => anyhow::bail!("unsupported provider '{other}'"),
    }
}
```

This returns `OpenAiProvider` for now because OpenAI is the only implemented provider. When future providers are added, the next refactor should change the return type to a router enum or boxed trait object. Do not build that full future-provider abstraction until there is a second provider implementation.

Update `src/main.rs` to use `build_provider` instead of inline OpenAI-only validation/construction. Keep unsupported providers rejected at startup.

**Verify**: `cargo test --all` should exit 0.

### Step 5: Make `/model` switch provider and model

In `src/tui.rs`:

1. Change `execute_model_command` to pass direct args through `catalog::resolve_model_selection`.
2. Change picker submission so it parses the canonical `provider/model` value into a `ModelSelection`.
3. Replace `select_model(model, ...)` with `select_model(selection, ...)` or `select_provider_model(provider, model, ...)`.
4. In the selection function:
   - Build a new provider using `build_provider(&provider, &model, &self.info.auth, reasoning_config_value(&self.info.reasoning_effort), reasoning_config_value(&self.info.reasoning_summary))`.
   - Call `agent.replace_provider(new_provider)`.
   - Set `self.info.provider = provider.clone()` and `self.info.model = model.clone()`.
   - Load config, set both `config.provider` and `config.model`, and save.
   - Insert a notice like `model switched to openai/gpt-5.5 and saved to config`.
   - Set status to `model: openai/gpt-5.5`.

Avoid duplicating `reasoning_config_value` if it is private in `main.rs`; either move that helper to a shared module or replicate a tiny local helper in `tui.rs`. Prefer moving it to the new provider module if simple.

Error behavior:

- If direct args are ambiguous, insert an `Entry::Error` explaining that the model exists under multiple providers and the user should run `/model provider/model`.
- If provider construction fails because auth is missing, keep the old provider active, do not save config, and insert the error.
- If config saving fails after the provider is replaced, show the existing style of warning, but mention both provider and model.

**Verify**: Add or update unit tests for command parsing/selection helpers where possible, then run `cargo test --all`.

### Step 6: Remove or de-emphasize `OpenAiProvider::list_models`

Since `/model` no longer uses provider-side listing, decide one of these two approaches:

- Preferred: remove `OpenAiProvider::list_models`, `list_openai_models`, `ModelsResponse`, and `ModelInfo` if no other code uses them.
- Acceptable: keep them private/dead-code-free only if another call site needs them. Do not leave public methods that trigger Codex listing failures.

If removing causes clippy warnings or test failures, fix the call sites rather than restoring remote listing for `/model`.

**Verify**: `rg -n "list_models|list_openai_models|ModelsResponse|ModelInfo" src` should show no stale unused dynamic-listing code, unless you intentionally kept a used helper with a clear reason.

### Step 7: Update documentation

Update `docs/interactive-tui.md`:

- Change `/model [model]` row to say it opens Rho's static cross-provider model catalog.
- Document direct syntax: `/model provider/model`.
- Mention bare model ids work only when unique, and may stay on the current provider for uncataloged models as an escape hatch.

Update `docs/guide/index.md`:

- In the config section, explain that `provider` and `model` are switched together by `/model`.
- In the Codex section, state that `/model` uses Rho's static catalog and does not query Codex for model listing.

**Verify**: `npm run docs:build` should exit 0.

### Step 8: Full verification

Run:

1. `cargo fmt --all -- --check`
2. `cargo test --all`
3. `cargo clippy --all-targets --all-features -- -D warnings`
4. `npm run docs:build`

All must exit 0.

## Test plan

Add tests for `src/model/catalog.rs` covering:

- `available_models("codex")` includes `openai/gpt-5.5` and does not call a provider.
- `resolve_model_selection("openai/gpt-5.5", "openai", "codex")` returns provider `openai`, model `gpt-5.5`.
- Bare unique model resolves to its catalog provider.
- Bare uncataloged model resolves to the current provider and sets `from_catalog = false`.
- Ambiguous bare model returns an ambiguity error once the test catalog contains two entries with the same model under different providers. If the production catalog only has one provider today, make the resolver accept a slice for testability or test a small private helper with a custom slice.

Existing full-suite tests should catch compile breakage from `TuiInfo` and provider factory changes.

## Done criteria

All must hold:

- [ ] `/model` no longer queries OpenAI `/models` or Codex model-list endpoints.
- [ ] `/model` picker is populated from a static catalog and shows all catalog entries for implemented providers and the active auth mode.
- [ ] Picker selection switches and persists both `provider` and `model`.
- [ ] `/model provider/model` switches and persists both `provider` and `model`.
- [ ] `/model <bare-model>` works when unique, errors when ambiguous, and preserves a current-provider escape hatch for uncataloged model ids.
- [ ] Codex auth cannot produce the old error `model listing is not available for Codex auth yet` from the `/model` picker.
- [ ] New catalog resolver tests pass.
- [ ] `cargo fmt --all -- --check` exits 0.
- [ ] `cargo test --all` exits 0.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `npm run docs:build` exits 0.
- [ ] `plans/README.md` marks plan 001 superseded/rejected and this plan updated appropriately.

## STOP conditions

Stop and report back if:

- A second provider implementation already exists by the time you start. In that case, the provider factory likely needs a router enum or boxed trait object rather than returning `OpenAiProvider`.
- The code at `src/tui.rs:834-852` no longer calls provider-side listing and has already been refactored around a catalog.
- The maintainer wants future-provider entries to be selectable before provider implementations exist. That would create runtime failures and needs a product decision.
- Switching provider would require dropping conversation history. This plan assumes the same `Agent` history remains valid across provider changes.
- Any verification command fails twice after reasonable fixes.

## Maintenance notes

- Treat `MODEL_CATALOG` as Rho's source of truth for picker-visible models. Future provider PRs should add the provider implementation and catalog entries together.
- Keep catalog entries evidence-based. Do not add guessed model IDs.
- The first implementation can keep `Agent<OpenAiProvider>` because only OpenAI exists today. When a second provider lands, introduce a `ProviderRouter` enum implementing `ModelProvider` and make `build_provider` return that router.
- Reviewers should scrutinize that config is only saved after successful provider construction, so `/model unsupported/model` does not leave the user with a broken persisted config.
