# SDK providers

## Current provider surface

`ModelProvider` is the provider extension point. It is object-safe, `Send + Sync`, and returns explicit `Send` futures. A provider reports an exact `ModelIdentity` containing provider, API, and model names, and receives a borrowed `ModelRequest` for each turn.

The current core crate includes `ScriptedProvider` for deterministic examples and tests. Production provider implementations used by the Rho application are not automatically exposed or configured by `rho-sdk`, and the SDK has no provider Cargo features today. Do not infer SDK availability from the application's [provider list](/authentication-and-models#providers).

## Implementor contract

A provider implementation must:

1. return a stable, exact identity for replay decisions
2. accept provider-neutral history and tool schemas without mutating session history
3. normalize output into `ModelResponse`, `ModelEvent`, `ModelUsage`, and sanitized `ProviderError` values
4. cooperate with the request's `CancellationToken`
5. avoid blocking the async runtime thread
6. apply bounded buffering and backpressure when bridging another streaming API
7. keep credentials, authorization headers, signed URLs, raw secret-bearing payloads, and transport-specific errors out of public `Debug`, events, diagnostics, and error messages
8. scope opaque provider-native replay data to the exact provider/API/model identity that produced it

`send_turn` is the non-streaming primitive. `send_turn_stream` may be overridden for streaming. Its default implementation invokes `send_turn` while observing cancellation. Streaming providers send semantic `ModelEvent` values through the supplied bounded sender and still return a complete normalized response. The final response, not accumulated deltas, is the authoritative completed turn.

## Credentials and endpoints

The core SDK does not acquire credentials. It does not read provider environment variables, Rho config files, or an operating-system credential store. An embedding host should construct its provider with an injected secret value or a narrow credential adapter.

Credential-bearing provider structs need custom `Debug` implementations that omit or redact values. Errors should classify authentication, rate limiting, transport, invalid responses, interruption, and other failures without embedding raw response bodies. Endpoint overrides, proxy behavior, TLS roots, timeouts, and retry policy belong to the adapter and must be explicit.

The Rho CLI has separate application-level authentication behavior documented in [authentication and models](/authentication-and-models). That behavior is not an ambient SDK default.

## Streaming and cancellation

The provider receives the run's shared cancellation token. The runtime also races the provider future against cancellation, so it stops polling the future when cancelled. Providers that create child tasks, network streams, or subprocesses remain responsible for propagating cancellation and cleaning up those resources when their future is dropped.

Provider events are forwarded in arrival order into the run event stream. Usage events are accumulated across steps. Raw reasoning is emitted as ephemeral `ReasoningDelta` events but is not copied into completed assistant history or snapshots. Provider-produced reasoning summaries may be persisted.

## Provider-native replay and handoff

`ProviderContextBlock` can contain opaque provider-native data. Each block is tagged with the exact `ModelIdentity` that produced it. Canonical `ModelRequest` history still contains these tagged blocks so an adapter can lower enriched messages. The adapter must use the handoff helpers or equivalent exact-identity filtering before constructing an upstream wire request:

- exact provider/API/model identity permits replay
- a changed provider, API, or model requires the adapter to omit incompatible blocks from the upstream request
- portable assistant content remains available
- `Session::replace_provider` returns a `HandoffReport` so the host can surface omissions

Opaque blocks may still exist in session history and snapshots. Treat their `data` as sensitive provider content, do not render it by default, and apply retention and encryption policy. Identity checks protect compatibility, not confidentiality.

## Stable versus upstream-dependent behavior

The 1.0 SDK contract can stabilize provider-neutral types, trait signatures, event ordering, cancellation propagation, error categories, identity matching, and handoff rules. It cannot guarantee behavior controlled by an upstream service, including:

- model availability, aliases, context limits, prices, or supported reasoning levels
- token accounting precision or when usage arrives
- streaming chunk boundaries and latency
- provider-native context format or replay longevity
- rate limits, quotas, authentication flows, and error wording
- tool-call quality, schema adherence, or image support
- server-side safety filtering, retention, or data residency

Provider-private wire formats are not part of the SDK compatibility promise. Adapters must normalize upstream changes without leaking private response types into the public API. Applications that depend on a specific service behavior should pin and test the adapter and surface service degradation independently of SDK SemVer.

## Test expectations

Use `ScriptedProvider` for downstream tests that must not require network credentials. A production adapter should have contract coverage for final text, streaming, tools, usage, malformed responses, cancellation, sanitized errors, and provider-native replay. Live credentialed tests may supplement those checks, but should not be required for deterministic SDK consumers.
