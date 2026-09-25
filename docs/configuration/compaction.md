# Auto compaction

Parent: [Configuration](/configuration).

`auto_compact` summarizes older conversation history when the estimated context approaches the model's effective window. It is on by default. Change it under `/config` → **Context & limits**, or in `[compaction]`. To opt out, set:

```toml
[compaction]
auto_compact = false
```

Models without a known context window get no automatic compaction or overflow recovery, whatever this setting says. Use `/compact` for them.

`compact_threshold_percent` is the trigger. `compact_target_percent` is the post-compaction target as a percent of the effective window. The target must stay below the threshold. A value at or above the threshold is clamped to one below it on load or save. Rho keeps the recent verbatim tail by token budget and safe tool-call boundaries, not by message count.

If the provider rejects a request as larger than the model's context window, Rho compacts and retries that request once. This only happens when `auto_compact` is on. Recovery uses the same half-current retention cap as `/compact`, so it still shrinks history when the configured window is larger than the provider's real limit. If compaction does not shrink the context, or the retry overflows again, the turn fails with the provider error. Recovery is skipped while background tool jobs are pending.

`/config` changes apply when the session is idle, before the next automatic check or model turn. Edits during a model turn or compaction wait for that operation to finish. Applying settings keeps the current provider-calibrated context estimate. External file edits require a restart.

## What the status line counts

The status line and the checks before prompts and provider steps use the same accounting. After a successful response, Rho uses that request's provider-reported prompt tokens, including cache hits, as a baseline, then estimates messages added since. It does not add prompt usage across requests.

The baseline survives ordinary turns and background metadata refreshes that leave model settings unchanged. History replacement, compaction, or a model or tool change invalidates it. It is not saved. A resumed session uses the local estimate until its first successful response reports usage. Provisional streaming usage from a failed or cancelled request does not replace the baseline.

Text-summary compaction converts the retained-tail budget into local-estimator units using the observed token ratio. The target is approximate. Summary length, fixed instructions, and indivisible tool-call groups can keep the result above it.

## Diagnostics

`/info` includes compaction counts and decisions. For structured output, ask the agent to call `rho` with `action = "compaction"`. The report includes calibrated and local counts, the provider request baseline, window, threshold, target, and the last idle and SDK checks with skip reasons.

`Last tier` (`last_tier` in structured output) shows which tier the last compaction used, `elision`, `native`, `text_summary`, or `unchanged`, and how many tool results it elided.

A check that requests compaction is not proof that it finished. `Completed` counts committed compactions, including unchanged results. `Last completed` shows the last before and after local token counts and whether they decreased, stayed the same, or increased. This survives resume and appears under `completed` in structured output. Failed or cancelled attempts do not replace the last committed result. Their `compact` card shows the error or cancellation. These diagnostics contain token counts and configuration, not conversation text.

## Tool-result elision

Every compaction first tries a cheap tier that needs no model request. Rho replaces the content of old tool results outside the recent tail with a short stub:

```text
[elided tool result: read_file path=src/main.rs · ok · 18342 bytes · recall_id=r3f9c0a1b2d4e5f60; fetch the text with the sessions tool, action=recall]
```

Oldest results go first, and elision stops once the context reaches the target. Results under 1 KiB stay verbatim; across local sessions they are about 38% of results but only about 5% of tool-result bytes. Tool results inside the recent tail are never elided. Each tool call keeps exactly one result with the same ID, so the history stays valid for every provider. Images attached to an elided result are removed with it, and the stub counts them.

If elision alone reaches the target, Rho commits the elided history and makes no model request. Otherwise native or text-summary compaction runs on the elided history. Like any compaction, elision invalidates the provider prompt cache from the first changed message on.

The model can fetch an elided result with the `sessions` tool, `action = "recall"`, and the stub's `recall_id`. Recall reads the current session's saved transcript, returns the original text in character windows (`start`, `chars`) within the tool output limit, and fails with an error for unknown IDs. Recall IDs derive from the tool call ID and content, so they stay stable across resume and branches. Recalled text is marked as untrusted tool output. Sessions without a saved transcript cannot recall.

## Which compactor runs

For `openai-codex` and API-key `openai`, Rho prefers OpenAI server-side compaction. API-key `openai` calls `POST /responses/compact`. `openai-codex` sends a normal streaming `POST /responses` ending with a `compaction_trigger` item, because the Codex backend no longer serves `/responses/compact`. That path returns only the encrypted compaction item, so Rho keeps system prompts and the newest user messages (up to about 64k tokens) itself. Both use the Responses API so the encrypted artifact stays replayable. The threshold still decides when auto compaction runs. `compact_target_percent` applies only if that path falls back to text-summary compaction. Catalog gateways that reuse the Responses shape, such as `opencode-go`, do not serve that endpoint, so they go straight to text-summary compaction.

xAI has its own server-side compact path. See [xAI](/providers/xai).

## Manual compact

`/compact` ignores `compact_target_percent` when that target would keep everything. The retained tail is capped at half the current estimated context, so an explicit request can remove history even on a large-window model that has not hit the auto threshold. Interactive pre-prompt auto-compaction uses this same task and cap. Automatic compaction between SDK provider steps uses the configured window target. An unchanged compactor result preserves a still-valid provider baseline instead of dropping the count back to the local estimate.

Auto compaction changes only future model context. Session files stay append-only. They keep the original transcript, then append a replacement-history entry used for resume. It is not a privacy or deletion feature. See [Sessions](/sessions#compaction-and-transcript-history).

Model metadata supplies the window when available. Override it in `~/.rho/models.toml`. See [Local model metadata](/configuration#local-model-metadata).
