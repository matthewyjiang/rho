# SDK public API inventory

This inventory records the intentional `rho-sdk` 1.0 surface. Items not listed
here are implementation details even when they are `pub(crate)` inside the
workspace. The application provider transports, login flows, `Config`, OS
credential store, TUI, and built-in coding tools are not SDK exports.

## Crate-root runtime surface

- Runtime construction: `Rho`, `RhoBuilder`, `SystemPrompt`, `ShutdownOutcome`
- Sessions and runs: `SessionOptions`, `Session`, `SessionState`, `UserInput`,
  `Run`, `RunEvent`, `RunOutcome`, `StopReason`
- Cancellation and reasoning: `CancellationToken`, `ReasoningLevel`,
  `ParseReasoningLevelError`
- IDs: `SessionId`, `RunId`, `ToolCallId`, `HostInputId`, `Revision`, `InvalidId`
- Host input: `HostChoice`, `HostQuestion`, `SelectionMode`, `HostInputRequest`,
  `HostInputResponse`
- Diagnostics: `DiagnosticsSnapshot`, `PromptSource`, `PromptSourceKind`
- Errors: `Error`, `ProviderError`, `ProviderErrorKind`, `Retryability`
- Secrets: `SecretString`
- Compaction: `CompactionPolicy`, `CompactionRequest`, `CompactionOutput`,
  `CompactionOutcome`, `CompactionState`, `CompactionTrigger`, `Compactor`,
  `CompactionFuture`, `ScriptedCompactor`
- Persistence: `SessionSnapshot`, `InMemorySessionStore`,
  `SESSION_SNAPSHOT_SCHEMA_VERSION`
- Workspace and approval contracts: `Workspace`, `CapabilityRequest`,
  `PolicyDecision`, `WorkspacePolicy`, `DenyAllPolicy`, `ScopedWorkspacePolicy`,
  `ApprovalRequest`, `ApprovalDecision`, `ApprovalHandler`, `ApprovalFuture`,
  `DenyApprovals`
- Tool run results re-exported at the root: `ToolCompletion`, `ToolFailure`

## `rho_sdk::model`

The intentional provider-neutral model exports are `ToolSpec`, `ToolCall`,
`ToolResult`, `PartialToolCall`, `ContentBlock`, `ImageContent`, `Message`,
`AssistantMessage`, `AbortedAssistant`, `ModelIdentity`,
`ProviderContextBlock`, `ModelRequest`, `ModelResponse`, `ModelEvent`,
`ModelUsage`, `ContextUsage`, and `ContextUsageSource`.

These types are serialized history and provider DTOs rather than open-ended
policy enums. Their current public fields and enum variants are intentional for
wire compatibility and ergonomic provider implementations. Adding a required
field or variant is treated as a source-breaking change. Compatible serialized
additions must be optional/defaulted, and source-level additions wait for a
major release or a replacement non-exhaustive contract. Host-facing policy,
event, error, and lifecycle enums are `#[non_exhaustive]` where adding facts is
expected.

Provider-native context is opaque JSON scoped by exact `ModelIdentity`. It must
not contain credentials. Reasoning text is not serialized in snapshots; see the
persistence contract for the replay and redaction invariants.

## `rho_sdk::provider`

The provider extension surface is `ModelProvider`, `ProviderFuture`,
`ProviderEventSender`, `ProviderEventReceiver`, and `provider_event_channel`.
`ScriptedProvider`, `ScriptedTurn`, and `RecordedModelRequest` are intentional
downstream test support.

`ModelProvider` is object-safe and requires `Send + Sync`; every returned future
is `Send`. Providers receive immutable request snapshots, own no session
mutation, and must cooperate with cancellation. Credentials are constructor
inputs to application adapters and never appear in `ModelRequest`, events,
diagnostics, snapshots, or provider identities.

## `rho_sdk::tool`

The tool extension surface is `Tool`, `ToolFuture`, `ToolRegistry`,
`DuplicateToolName`, `ToolInvocation`, `ToolContext`, `ToolOutput`, `ToolError`,
`ToolErrorKind`, `ToolMetadata`, `OperationKind`, `ToolProgress`,
`ToolProgressSender`, `ToolProgressReceiver`, and `tool_progress_channel`.
`ScriptedTool` and `ScriptedToolOutcome` are intentional downstream test
support.

Tools are object-safe and `Send + Sync`. Structured metadata is a runtime fact,
not terminal presentation. Capability authorization and cancellation remain
explicit in `ToolContext`.

## Evolution and redaction decisions

- Open-ended host contracts are non-exhaustive. Closed serialized model DTOs
  remain exhaustive and are versioned as source contracts.
- Builders use named methods for policy and optional values. Required provider
  and credential state is supplied before `build`, and endpoint URLs plus
  request timeouts are typed and validated.
- `SecretString` redacts both `Debug` and `Display`, requires an explicit expose
  operation, and does not implement serde serialization.
- Built-in provider conversion drops raw HTTP bodies, malformed payload text,
  request error details, I/O details, and credential-store details before
  creating public SDK errors.
- SDK diagnostics, events, requests, and snapshots have no credential fields.
  Arbitrary user, model, and tool content is application data and is not
  automatically rewritten, so hosts must apply their own content logging
  policy.

## Application compatibility shims

Two private application shims remain, both tracked for removal by issue #256:

1. `providers::build_sdk_provider(provider, model, reasoning)` supports direct
   TUI provider replacement and delegates immediately through the explicit
   application credential source and provider builder.
2. `providers::build_provider(provider, model, reasoning)` supports the private
   TUI goal-provider trait while delegating through the same explicit boundary.

`providers::sdk_adapter` remains the transport compatibility layer until all
application provider consumers use the public SDK trait. It contains no public
SDK export and has no independent credential lookup. New application code must
use `ProviderBuildOptions`, an explicit `ProviderCredentialSource`, and
`build_sdk_provider_with_source`.
