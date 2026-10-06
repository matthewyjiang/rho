# Auto compaction

Parent: [Configuration](/configuration).

`auto_compact` summarizes older conversation history when the estimated context approaches the model's effective window. It is on by default. Change it under `/config` → **Context & limits**, or in `[compaction]`. To opt out, set:

```toml
[compaction]
auto_compact = false
```

Models without a known context window get no automatic compaction or overflow recovery, whatever this setting says. Use `/compact` for them.

`compact_threshold_percent` is the trigger. `compact_target_percent` is the post-compaction target as a percent of the effective window. The target must stay below the threshold. A value at or above the threshold is clamped to one below it on load or save. Rho keeps the recent verbatim tail by token budget and safe tool-call boundaries, not by message count.

If the provider rejects a request as larger than the model's context window, Rho compacts and retries that request once. This only happens when `auto_compact` is on. Recovery uses the same half-current retention cap as `/compact`, so it still shrinks history when the configured window is larger than the provider's real limit. If compaction does not shrink the context, or the retry overflows again, the turn fails with the provider error.

While background tool jobs run, automatic compaction and overflow recovery summarize only the history before the earliest running tool call. That call and everything after it stay as they are, so the job's result still lines up with its call when it arrives. Compaction is skipped when nothing new sits before that call: the job started in the conversation's first reply, or that history was already compacted while the same jobs ran.

`/config` changes apply when the session is idle, before the next automatic check or model turn. Edits during a model turn or compaction wait for that operation to finish. Applying settings keeps the current provider-calibrated context estimate. External file edits require a restart.

## What the status line counts

The status line and the checks before prompts and provider steps use the same accounting. After a successful response, Rho uses that request's provider-reported prompt tokens, including cache hits, as a baseline, then estimates messages added since. It does not add prompt usage across requests.

The baseline survives ordinary turns and background metadata refreshes that leave model settings unchanged. History replacement, compaction, or a model or tool change invalidates it. It is not saved. A resumed session uses the local estimate until its first successful response reports usage. Provisional streaming usage from a failed or cancelled request does not replace the baseline.

Text-summary compaction converts the retained-tail budget into local-estimator units using the observed token ratio. The target is approximate. Summary length, fixed instructions, and indivisible tool-call groups can keep the result above it.

## Diagnostics

`/info` includes compaction counts and decisions. For structured output, ask the agent to call `rho` with `action = "compaction"`. The report includes calibrated and local counts, the provider request baseline, window, threshold, target, and the last idle and SDK checks with skip reasons.

The `Last run`, `Run cost`, and `After run` rows (`last_compaction` in structured output) describe the latest compactor call, committed or not:

- **Last run**: trigger (`automatic`, `manual`, or `context_overflow`; idle auto-compaction before a prompt reports `manual`), outcome (`completed`, `failed`, or `cancelled`), tier (`elision`, `native`, `text_summary`, or `unchanged`), the text-summary request path (`session_history`, `transcript`, or `summarizer`, the last one tried), and how many tool results were elided.
- **Run cost**: wall-clock latency, the `provider/model` that served the last request, and provider-reported prompt, output, and cache-read tokens and cost, summed over every request the compaction sent, including failed attempts.
- **After run**: the first provider-reported prompt size after the commit, and a re-read count. For the next 24 tool calls, Rho counts calls that repeat a `read_file` path or shell command whose result the compaction removed, either summarized away or elided. 24 is the p75 gap between a call and its repeat across 135 local sessions. The count is a trend signal for comparing tiers, not a pass/fail gate: some repeats are legitimate, for example after the file changed. Only the interactive TUI collects these follow-up numbers. Headless and ACP runs record the rest.

Each compactor call is also saved as a row in the usage ledger's `compaction_events` table, so numbers can be compared across sessions. See [usage ledger](/usage-ledger). To measure what a compaction loses rather than what it costs, see the [compaction replay eval](/development#compaction-replay-eval).

A check that requests compaction is not proof that it finished. `Completed` counts committed compactions, including unchanged results. `Last completed` shows the last before and after local token counts and whether they decreased, stayed the same, or increased. This survives resume and appears under `completed` in structured output. Failed or cancelled attempts do not replace the last committed result. Their `compact` card shows the error or cancellation. These diagnostics contain token counts and configuration, not conversation text.

## Tool-result elision

Compaction first tries a cheap tier that needs no model request. Rho replaces the content of old tool results outside the recent tail with a short stub:

```text
[elided tool result: read_file path=src/main.rs · ok · 18342 bytes · recall_id=r3f9c0a1b2d4e5f60; fetch the text with the sessions tool, action=recall]
```

Oldest results go first, and elision stops once the context reaches the target. Results under 1 KiB stay verbatim; across local sessions they are about 38% of results but only about 5% of tool-result bytes. Tool results inside the recent tail are never elided. Each tool call keeps exactly one result with the same ID, so the history stays valid for every provider. Images attached to an elided result are removed with it, and the stub counts them.

If elision alone reaches the target, Rho commits the elided history and makes no model request. Otherwise native or text-summary compaction runs on the elided history, so the summary sees the stubs rather than the original tool output. This keeps a large history from overflowing the summary request. The tradeoff is that summaries describe old tool results only by tool, arguments, and status. Like any compaction, elision invalidates the provider prompt cache from the first changed message on.

Before a stub enters the history, Rho saves the original under the session folder in `recall/<recall_id>.json`. The model fetches it with the `sessions` tool, `action = "recall"`, and the stub's `recall_id`. Recall returns the original text in character windows (`start`, `chars`) within the tool output limit, marks it as untrusted tool output, and fails with an error for unknown IDs. Only text is recallable; images removed with an elided result are not. The recall files sit outside the workspace, so checked permission modes may ask for approval, as they do for other `sessions` reads. Recall IDs derive from the tool call ID and content, so they stay stable across resume and branches.

Elision runs only when the stubs can be recalled. It is skipped, and compaction goes straight to native or text-summary compaction, when:

- the agent does not have the `sessions` tool (for example the built-in `explorer` and `reviewer`)
- the session is not saved: `--no-save`, subagents, and automation runs
- the session uses a legacy flat transcript without a session folder
- saving the originals fails

## Text summaries

When elision is not enough and native compaction is unavailable, Rho asks the session model for a summary, unless a [summarizer model](#summarizer-model) is set. The summary always uses the same sections: original request, constraints and preferences, decisions and rationale, files touched and their current state, commands run and test results, errors and fixes, open tasks, and the exact next step. The model may think in an `<analysis>` block first. Rho removes that block before the summary enters context.

The summary goes into history as a compaction summary, labeled by what caused it: automatic, manual (`/compact`), or context overflow. Providers receive it as a user-role message that says it is not a new user message.

A later compaction updates the earlier summary instead of summarizing it again. The earlier summary goes to the model as a previous summary to revise with the newer turns, so detail is not lost at each round.

Two user messages stay verbatim outside the summary, each only if its estimate is at most 2,048 tokens:

- The first user turn, kept ahead of the summary. A larger first turn is only summarized.
- The latest user message, restated after the summary when it would otherwise fall outside the recent tail. This keeps the instruction for the current turn, such as a `/goal` prompt, when compaction runs in the middle of a long turn.

If those messages are the only history left to remove, they are summarized too.

### Prompt cache reuse

The session-model summary request resends the conversation as the session model last saw it, with the same system prompt, messages, tool definitions, reasoning level, service tier, and prompt cache key, then adds one user message asking for the summary. The provider has already cached most of that prefix, so the request is billed mostly as cache reads. Its usage is recorded with purpose `compaction`, including cache reads, so it shows up in `/spend` and `/info`.

That request resends old tool results in full, even when [elision](#tool-result-elision) stubbed them, because stubs would change the cached prefix. The instruction appended after that history names the span compaction will delete, so the model does not summarize the whole conversation or skip the turns that are about to disappear. Rho renders the history as one transcript instead, with elided stubs and no service tier, when:

- the full request would not leave room for the summary in the model's window
- the provider rejects it, for example as too large
- compaction is recovering from a context overflow
- the model calls a tool instead of answering, which Rho never executes
- the reply has no summary text

Whether the cache hits depends on the provider's caching rules, such as minimum prefix size, cache lifetime, and whether it caches at all.

### Summarizer model

Set a model for the `compaction` internal agent to write summaries on a cheaper model. Run `/agents`, select `compaction`, and pick a model, or edit config. Model aliases work here.

```toml
[internal_agents.compaction]
provider = "openai"
model = "gpt-5.6-luna"
auth = "api-key"
reasoning = "low" # optional; defaults to low
```

A different model cannot reuse the session's prompt cache, so it always gets the rendered transcript and does not inherit the session's service tier. A smaller model may also drop more detail from the summary. If the summarizer fails, for example because its credentials are missing, its window is too small, or it returns no summary, Rho logs a warning and tries the cached session-history request, then the transcript. A failed summarizer does not cost more than compaction with no summarizer, when the session-history request fits. **Use conversation model** removes the override, and compaction goes back to the session model at the session's reasoning level. In the interactive TUI, changes apply when the session is next idle. Other hosts pick them up on the next start.

Native compaction ignores this setting.

## Which compactor runs

For `openai-codex` and API-key `openai`, Rho prefers OpenAI server-side compaction. API-key `openai` calls `POST /responses/compact`. `openai-codex` sends a normal streaming `POST /responses` ending with a `compaction_trigger` item, because the Codex backend no longer serves `/responses/compact`. That path returns only the encrypted compaction item, so Rho keeps system prompts and the newest user messages (up to about 64k tokens) itself. Both use the Responses API so the encrypted artifact stays replayable. The threshold still decides when auto compaction runs. `compact_target_percent` applies only if that path falls back to text-summary compaction. Catalog gateways that reuse the Responses shape, such as `opencode-go`, do not serve that endpoint, so they go straight to text-summary compaction.

SpaceXAI has its own server-side compact path. See [SpaceXAI](/providers/xai).

## Manual compact

`/compact` ignores `compact_target_percent` when that target would keep everything. The retained tail is capped at half the current estimated context, so an explicit request can remove history even on a large-window model that has not hit the auto threshold. Interactive pre-prompt auto-compaction uses this same task and cap. Automatic compaction between SDK provider steps uses the configured window target. An unchanged compactor result preserves a still-valid provider baseline instead of dropping the count back to the local estimate.

Auto compaction changes only future model context. Session files stay append-only. They keep the original transcript, then append a replacement-history entry used for resume. It is not a privacy or deletion feature. See [Sessions](/sessions#compaction-and-transcript-history).

Model metadata supplies the window when available. Override it in `~/.rho/models.toml`. See [Local model metadata](/configuration#local-model-metadata).
