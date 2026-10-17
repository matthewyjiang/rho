# Request compression

Long conversations resend system instructions, tool schemas, source files, and tool results on each turn. Rho gzip-compresses large JSON inference requests where the **receiving endpoint** is known to accept `Content-Encoding: gzip`.

This is request-body compression, not `Accept-Encoding` response negotiation. Streaming responses remain SSE. Codex WebSocket messages are not HTTP request bodies and are unchanged.

## Behavior

- Automatic gzip on verified first-party Anthropic Messages, Google Gemini Generate Content, xAI REST, and TypeSafe System One endpoints, including streaming turns where applicable.
- Requests smaller than **16 KiB (16,384 serialized bytes)** stay JSON. Compression uses the fast gzip level, and the compressed body is sent only if it is smaller.
- `Content-Type: application/json`, credentials, provider headers, and SSE negotiation are preserved. The HTTP client computes the compressed `Content-Length` rather than retaining the original length.
- Existing content encodings and streaming request bodies are not recompressed.
- Support is checked against the actual HTTPS host, port, and inference path. A custom endpoint does **not** inherit support from its provider name, models.dev catalog, or OpenAI-compatible dialect.
- Unsupported and unverified endpoints keep ordinary JSON. Rho does not replay a generation after a speculative compressed request: an ambiguous failure could otherwise duplicate a billed turn.

The threshold rationale and performance measurements are recorded in [PR #1524](https://github.com/matthewyjiang/rho/pull/1524).

## Provider support audit

Gzip support must be established for **requests**. A server emitting gzipped responses, advertising `Accept-Encoding`, or returning the same early authentication failure for both payloads is not proof that it decodes gzip requests.

[T3 Code #4788](https://github.com/pingdotgg/t3code/pull/4788) compresses negotiated JSON **responses** carrying thread snapshots. Outbound model requests need their own endpoint compatibility check; there is no equivalent universal `Accept-Encoding` negotiation for them.

| Provider / endpoint | Finding | Rho behavior |
| --- | --- | --- |
| Anthropic `api.anthropic.com/v1/messages` | Confirmed: [Claude Code's documented gzip request behavior](https://code.claude.com/docs/en/env-vars), restricted to `api.anthropic.com`. | Automatic gzip at/above the threshold. |
| Google Gemini `generativelanguage.googleapis.com/v1beta/models/*:generateContent` and `:streamGenerateContent` | Confirmed decoder with malformed-JSON controls (below), including the SSE endpoint. [Generate Content API](https://ai.google.dev/api/generate-content). | Automatic gzip at/above the threshold. |
| xAI `api.x.ai/v1/responses`, `/responses/compact`, `/chat/completions` | Confirmed decoder: actual gzip reaches matching plaintext JSON/schema validation; corrupt gzip fails body buffering. [API specification](https://docs.x.ai/openapi.json). | Automatic gzip at/above the threshold, including native HTTP compaction. |
| TypeSafe `api.typesafe.ai/v1/systemone` | Explicit [request compression contract](https://docs.typesafe.ai/api#compression), including gzip/x-gzip and zstd. | Automatic gzip at/above the threshold. Existing request-size budgets still apply to the serialized JSON before compression. |
| OpenAI `api.openai.com/v1/responses`, `/responses/compact`, `/chat/completions` | **Unverified**: probes stop at authentication; no affirmative gzip contract in the [Responses](https://developers.openai.com/api/reference/resources/responses/methods/create) or [Chat](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create) references. | Ordinary JSON. |
| Codex HTTP `chatgpt.com/backend-api/codex/responses` | **Gzip unverified**. The official client uses [zstd request encoding](https://github.com/openai/codex/blob/e16b631d08cdf82773831f9bbfb793da687d5299/codex-rs/http-client/src/request.rs#L196-L217), selected for eligible Codex backend authentication. | Ordinary JSON for HTTP fallback/remote compaction; gzip is not substituted for zstd. |
| Codex WebSocket `wss://chatgpt.com/backend-api/codex/responses` | HTTP body gzip does not apply. The official client negotiates [WebSocket `permessage-deflate`](https://github.com/openai/codex/blob/e16b631d08cdf82773831f9bbfb793da687d5299/codex-rs/codex-api/src/endpoint/responses_websocket.rs#L576-L582). | Unchanged JSON frames. WebSocket extension negotiation and HTTP zstd are separate work. |
| GitHub Copilot `api.githubcopilot.com/chat/completions` and token-selected hosts | **Unverified**: probes fail the Authorization check before decoding can be observed. [First-party transport](https://github.com/microsoft/vscode-copilot-chat/blob/5863f5a7088958050792b5dccbe8b46c6e13eccc/src/platform/networking/node/baseFetchFetcher.ts#L33-L40). | Ordinary JSON. |
| OpenRouter `openrouter.ai/api/v1/chat/completions` | **Unverified**: all controls stop at authentication; the [API contract](https://openrouter.ai/docs/api_reference/overview) does not establish request gzip. | Ordinary JSON. |
| Ollama local `/v1/chat/completions` | **Not decoded by inspected stock server**: [middleware handles zstd only](https://github.com/ollama/ollama/blob/2476d5ead197abe04629651028f83d48dc75d2b2/server/cloud_proxy.go#L74-L109), and gzip reaches [JSON binding](https://github.com/ollama/ollama/blob/2476d5ead197abe04629651028f83d48dc75d2b2/middleware/openai.go#L449-L455) undecoded. A configured LAN Ollama instance also rejected actual gzip at the gzip magic byte. | Ordinary JSON. |
| Ollama Cloud `ollama.com/v1/chat/completions` | **Unverified**: ordinary JSON, gzip, and corrupt gzip all return early 401s. Local-server behavior does not prove cloud ingress behavior. [Compatibility docs](https://docs.ollama.com/api/openai-compatibility). | Ordinary JSON. |
| Moonshot `api.moonshot.ai/v1/chat/completions` | **Unverified end-to-end**: valid gzip reaches authentication while corrupt gzip fails earlier, suggesting ingress recognition but not proving API decoding. [Endpoint](https://api.moonshot.ai/v1/chat/completions). | Ordinary JSON. |
| Kimi Code `api.kimi.com/coding/v1/chat/completions` | **Unverified**: all controls stop at authentication. [Endpoint documentation](https://www.kimi.com/code/docs/en/). Do not inherit Moonshot's partial evidence. | Ordinary JSON. |
| Poolside `inference.poolside.ai/v1/chat/completions` | **Rejects actual gzip in controlled probes**: plain invalid-model JSON reaches authentication, while the compressed equivalent gets `400 Invalid request body`. [API docs](https://docs.poolside.ai/api/overview). | Ordinary JSON. |
| Alibaba/Qwen Token Plan `token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/chat/completions` | **Unverified**: Rho's exact endpoint returns 401 for every control. [Token Plan docs](https://docs.qwencloud.com/token-plan/personal/token-plan-personal-quickstart). Other Alibaba/Qwen hostnames are not interchangeable capability evidence. | Ordinary JSON. |
| Meta `api.meta.ai/v1/chat/completions` | **Unverified**: all controls return `401 invalid_api_key`. [API docs](https://dev.meta.ai/docs/quickstart). | Ordinary JSON for both API-key and Muse authentication. |
| OpenCode Go `opencode.ai/zen/go/v1/{chat/completions,responses,messages}`; Zen `opencode.ai/zen/v1/{chat/completions,responses,messages}` | **Likely rejects (moderate confidence)**: [public proxy](https://github.com/anomalyco/opencode/blob/65932e870ca50d21886f9d0ee89c6603bac34a65/packages/console/app/src/lib/inference-proxy.ts#L17-L60) preserves compressed bytes and encoding, with no decompressor. Probes produce different 400/500 errors, but the downstream inference implementation is unavailable. | Ordinary JSON on every adapter; no inherited Anthropic/Responses capability. |
| MiniMax `api.minimax.io/anthropic/v1/messages` | **Unverified**: controls stop at authentication; the [Messages specification](https://platform.minimax.io/docs/api-reference/text-chat-anthropic) does not promise gzip. | Ordinary JSON. |
| Stock CLIProxyAPI, custom `/responses`, `/chat/completions`, `/messages` | **No gzip decoding in inspected source**: [Chat/Responses decoder](https://github.com/router-for-me/CLIProxyAPI/blob/911f49cbd49f4fe2516979754a7488a3fd14fe64/sdk/api/handlers/request_body.go#L35-L70) supports zstd, not gzip; [Messages reads raw bytes](https://github.com/router-for-me/CLIProxyAPI/blob/911f49cbd49f4fe2516979754a7488a3fd14fe64/sdk/api/handlers/claude/code_handlers.go#L72-L84). | Ordinary JSON. Deployment-specific decompression middleware could change support, but the provider name alone proves nothing. |
| Other custom OpenAI-compatible endpoints | **Deployment-specific**: a shared JSON schema is not a transport contract. | Ordinary JSON unless the actual endpoint matches the verified allowlist above. |

Audit snapshot: **2026-10-17 UTC**, covering all 17 built-in providers plus CLIProxyAPI, OpenCode Zen, and arbitrary custom hosts. No credentials were read and no authenticated generation was requested for the audit. Positive Google/xAI controls establish body decoding, not a completed authenticated generation or SSE stream. The body decoder acts before authentication/schema handling; streaming flags do not change the compressed JSON representation. Unknown results are deliberately described as **unverified**, not unsupported. Vertex AI and differently hosted proxies are not covered by the Gemini finding.

Header-only tests are insufficient: CLIProxyAPI can accept plain JSON incorrectly labelled `Content-Encoding: gzip` via its raw-JSON fallback while rejecting actual gzip bytes. The probes use actual compressed bodies and corrupt-gzip controls, not just the header.

### Verifying a decoder without a paid generation

For Gemini's unary and streaming endpoints, malformed JSON provides a useful control. With an invalid probe key, gzipped `{not-json` returns a JSON parse error quoting the **decompressed** text; gzipped `{}` advances to `API_KEY_INVALID`. A corrupt gzip stream instead fails payload parsing. This establishes request decoding, not successful authenticated inference.

Larger controls used a 1,048,605-byte malformed JSON object compressed to 1,084 bytes. Gemini's unary/streaming routes reached schema validation on the decompressed `padding` field. xAI create/compact reported the malformed `gzip_probe` value at plaintext column 1,048,604, demonstrating decoding beyond the production size threshold without requesting generation.

For example (repeat with `streamGenerateContent?alt=sse`):

```sh
printf '{not-json' | gzip -c | curl -sS \
  'https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent' \
  -H 'Content-Type: application/json' \
  -H 'Content-Encoding: gzip' \
  -H 'x-goog-api-key: invalid-probe' --data-binary @-
```
