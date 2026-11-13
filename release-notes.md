## Release preparation

This automated PR prepares the next Rho releases. The sections below list each package version, its generated release notes, and the version files that will be updated.

Merging this PR creates draft GitHub releases, validates and publishes the selected crates, publishes the GitHub releases, and builds the release artifacts.
---


<details><summary>rho-agent-tools: 2.0.0</summary>

## [2.0.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v1.16.3...rho-agent-tools-v2.0.0) (2026-11-13)


###   BREAKING CHANGES

* **hooks:** HookCapability::ExecuteProcess gains a requested_command field and HOOK_SCHEMA_VERSION is now 3.
* **sdk:** Message gains CompactionSummary, ModelContext, BoundaryInput, and ReasoningShift variants rendered through Message::wire(); ToolCall.execution replaces async marker blocks; ModelEvent gains ToolCallAccepted, Stopped, and ProviderContext with a ReplayScope; ProviderContextBlock.replay replaces the hard-coded Codex replay exception; the session snapshot schema is now 3. Saved 5.x history loads as typed entries.
* **sdk:** ToolResult carries status: ToolResultStatus and images instead of ok: bool; the tool image supplement message API is removed; ToolCompletion::Completed replaces Success and CompletedFailure; RunEvent::ToolStarted carries execution and replaces ToolDetached; ToolProgress uses new/with_metadata with getters. Saved 2.x sessions still load.
* **sdk:** ModelProvider has one send_turn_stream(request, ProviderTurnContext) and native_compact takes ModelRequestOptions; provider_steering_channel returns ProviderSteeringSender; StepStarted carries context_estimate; SteeringRetraction gains Delivered; InvalidConfiguration, Persistence, and Interrupted errors are struct variants with an optional source; host input and tool host builders use with_* setters and unsuffixed getters; AfterToolUsePayload carries capability and process.
* **sdk:** QuestionKind, DefaultSelection, HostInputSource, CompactorCancellationMode, HookSessionFailureKind, and ScriptedToolOutcome are now #[non_exhaustive]; exhaustive matches outside rho-sdk need a wildcard arm. Decision adapters should reject unknown question kinds with InvalidRequest::UnsupportedQuestionKind.
* **tools:** the hashline edit format and its `edit` tool are gone; `edit_tool = "hashline"` (global or per custom provider) now loads as `str_replace` with a migration warning. rho-agent-tools drops `EditFormat::Hashline`, the `hashline` module, `FileViewPolicy`/`FileViewStyle`, the public `rtk` module, the `Bash::new`/`PowerShell::new` RTK argument, and the `ToolCard` match_* fields, which move into the new `ToolBody::Search` variant alongside a new `ToolBody::Code` variant.

### Features

* **daemon:** catch client and daemon version skew at the handshake, not at the first failed request ([#1784](https://github.com/matthewyjiang/rho/issues/1784)) ([6230b86](https://github.com/matthewyjiang/rho/commit/6230b867f3fc2f30cfa5cc17433bb2acee4810aa))
* **daemon:** run a session's commands with its client's environment ([#1733](https://github.com/matthewyjiang/rho/issues/1733)) ([0833ae2](https://github.com/matthewyjiang/rho/commit/0833ae2a9b11bf3979fb1cbbee47668c3090ed46))
* drop computer use ([#1738](https://github.com/matthewyjiang/rho/issues/1738)) ([4ba3648](https://github.com/matthewyjiang/rho/commit/4ba36487c5d5c2ef3b37a89f212f8ce1e7effe35))
* **hooks:** show hooks the command the model requested when rtk rewrites it ([#1892](https://github.com/matthewyjiang/rho/issues/1892)) ([e2e01bd](https://github.com/matthewyjiang/rho/commit/e2e01bd35afe50f558efc5affae8fa0d8fe20712))
* **protocol:** send the TUI's tool cards on daemon tool calls ([#1754](https://github.com/matthewyjiang/rho/issues/1754)) ([9691ca0](https://github.com/matthewyjiang/rho/commit/9691ca0de58a11e6352e5e2a940ac9e56a48fc7e))


### Bug Fixes

* **daemon:** give each session its own client's credentials ([#1787](https://github.com/matthewyjiang/rho/issues/1787)) ([517312a](https://github.com/matthewyjiang/rho/commit/517312ac71c9571c8e9ae61afe718191e1ef0dd8)), closes [#1779](https://github.com/matthewyjiang/rho/issues/1779)


### Code Refactoring

* **sdk:** carry tool outcomes and images on tool results ([#1693](https://github.com/matthewyjiang/rho/issues/1693)) ([4d52a30](https://github.com/matthewyjiang/rho/commit/4d52a30c553665ebeeeff61a70906afd206fc413))
* **sdk:** collapse provider turn methods and finish builder renames ([#1691](https://github.com/matthewyjiang/rho/issues/1691)) ([233e1f9](https://github.com/matthewyjiang/rho/commit/233e1f92be1f556c3469b9f8ead1447ee9e10579))
* **sdk:** mark host-facing sdk enums non-exhaustive ([#1687](https://github.com/matthewyjiang/rho/issues/1687)) ([3bac6f5](https://github.com/matthewyjiang/rho/commit/3bac6f582ffbce71703e66b176163d5017ff92ab))
* **sdk:** type compaction summaries, model context, and stop reasons in history ([#1695](https://github.com/matthewyjiang/rho/issues/1695)) ([4f44921](https://github.com/matthewyjiang/rho/commit/4f4492157a2835077b131a1ec719faae79f51bc8))
* **tools:** remove the hashline edit format and the legacy rtk api ([#1692](https://github.com/matthewyjiang/rho/issues/1692)) ([f766d90](https://github.com/matthewyjiang/rho/commit/f766d90d46a7c614b2fc49d097e4ec9ef33680f1)), closes [#1684](https://github.com/matthewyjiang/rho/issues/1684)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-protocol bumped from 0.1.0 to 1.0.0
</details>

<details><summary>rho-coding-agent: 3.0.0</summary>

## [3.0.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.33.0...rho-coding-agent-v3.0.0) (2026-11-13)


###   BREAKING CHANGES

* **daemon:** Message::User, Message::Assistant and Message::BoundaryInput hold Arc<[ContentBlock]> instead of Vec<ContentBlock>; Message::ToolResult holds Arc<ToolResult>; Message::EnrichedAssistant and Message::AbortedAssistant hold Arc instead of Box. Build them with `.into()` and change a shared payload with Arc::make_mut.
* **sdk:** Message::User, Message::Assistant and Message::BoundaryInput hold Arc<[ContentBlock]> instead of Vec<ContentBlock>; Message::ToolResult holds Arc<ToolResult>; Message::EnrichedAssistant and Message::AbortedAssistant hold Arc instead of Box. Build them with `.into()` and change a shared payload with Arc::make_mut.
* **tui:** `RHO_TUI_DAEMON=0` no longer keeps a session in the TUI's own process; rho ignores the variable. MCP `sampling/createMessage`, which only the in-process session answered, is no longer answered.
* **hooks:** HookCapability::ExecuteProcess gains a requested_command field and HOOK_SCHEMA_VERSION is now 3.
* **sdk:** Message gains CompactionSummary, ModelContext, BoundaryInput, and ReasoningShift variants rendered through Message::wire(); ToolCall.execution replaces async marker blocks; ModelEvent gains ToolCallAccepted, Stopped, and ProviderContext with a ReplayScope; ProviderContextBlock.replay replaces the hard-coded Codex replay exception; the session snapshot schema is now 3. Saved 5.x history loads as typed entries.
* **sdk:** ToolResult carries status: ToolResultStatus and images instead of ok: bool; the tool image supplement message API is removed; ToolCompletion::Completed replaces Success and CompletedFailure; RunEvent::ToolStarted carries execution and replaces ToolDetached; ToolProgress uses new/with_metadata with getters. Saved 2.x sessions still load.
* **sdk:** ModelProvider has one send_turn_stream(request, ProviderTurnContext) and native_compact takes ModelRequestOptions; provider_steering_channel returns ProviderSteeringSender; StepStarted carries context_estimate; SteeringRetraction gains Delivered; InvalidConfiguration, Persistence, and Interrupted errors are struct variants with an optional source; host input and tool host builders use with_* setters and unsuffixed getters; AfterToolUsePayload carries capability and process.
* **sdk:** QuestionKind, DefaultSelection, HostInputSource, CompactorCancellationMode, HookSessionFailureKind, and ScriptedToolOutcome are now #[non_exhaustive]; exhaustive matches outside rho-sdk need a wildcard arm. Decision adapters should reject unknown question kinds with InvalidRequest::UnsupportedQuestionKind.
* **tools:** the hashline edit format and its `edit` tool are gone; `edit_tool = "hashline"` (global or per custom provider) now loads as `str_replace` with a migration warning. rho-agent-tools drops `EditFormat::Hashline`, the `hashline` module, `FileViewPolicy`/`FileViewStyle`, the public `rtk` module, the `Bash::new`/`PowerShell::new` RTK argument, and the `ToolCard` match_* fields, which move into the new `ToolBody::Search` variant alongside a new `ToolBody::Code` variant.
* **workflow:** workflow plan/status JSON no longer includes graph, graph_digest, or flat node status fields (read program/program_digest and state.scopes instead), root task instance IDs are now `s0.<definition>` with JSONL wire version 2, and plans and runs saved before Rho 2.15 must be inspected with Rho 2.x.
* **providers:** rho-providers removes run_codex_oauth_flow, run_xai_oauth_flow, run_openrouter_oauth_flow, the OAuth and Ollama device Browser error variants, start_ollama_device_login's open_browser argument, InteractiveUserAction, InteractiveLogin::user_action, accept_unconfirmed_login and the image_input lookup; start_interactive_login now takes BrowserAvailability (replacing start_interactive_login_with_availability), InteractiveLoginCompletion::Unconfirmed carries an UnconfirmedAuthentication handle to accept, and ModelMetadata gains image_input and reasoning_control. ModelMetadata, ProviderId, ProviderRuntime, ModelSelectionError, CatalogSdkAdapter and the new ReasoningControl are #[non_exhaustive], so downstream code must build ModelMetadata from Default and keep wildcard match arms.
* **subagents:** result.json now writes runtime_session_id and runtime_model instead of claude_session_id and claude_model (old files still read), and Antigravity agent definitions no longer accept the view_file and create_file tool aliases; use read_file and write_file.

### Features

* **acp:** label T3 subagent threads with the per-call model ([#1786](https://github.com/matthewyjiang/rho/issues/1786)) ([d9eafe4](https://github.com/matthewyjiang/rho/commit/d9eafe480d8baa7e02970e75169c219fc4144546))
* **acp:** report and cancel background work over ACP v2 ([#1947](https://github.com/matthewyjiang/rho/issues/1947)) ([bc47bd7](https://github.com/matthewyjiang/rho/commit/bc47bd773a8431638aac618dc558c8b3bfb201fa))
* **acp:** run T3 Code child threads from daemon sessions ([#1750](https://github.com/matthewyjiang/rho/issues/1750)) ([33f646f](https://github.com/matthewyjiang/rho/commit/33f646fb9a069e0e9fdbeabadac440f3c67147e0))
* **acp:** wake T3 Code threads for background results from daemon sessions ([#1755](https://github.com/matthewyjiang/rho/issues/1755)) ([924dd29](https://github.com/matthewyjiang/rho/commit/924dd29647237f786310683dd792a928be01bf90))
* **attach:** answer, steer, and cancel a daemon session's turn from the terminal ([#1727](https://github.com/matthewyjiang/rho/issues/1727)) ([f4fba7e](https://github.com/matthewyjiang/rho/commit/f4fba7e259ce6e6c1ae0a2f64be1bd2733f94bb5))
* **attach:** follow a daemon session's turn over the protocol ([#1726](https://github.com/matthewyjiang/rho/issues/1726)) ([c3bf68e](https://github.com/matthewyjiang/rho/commit/c3bf68e72a65b0b0ddbf72f04ced8b9c2fa25e1e))
* **context:** timestamp user messages and assistant text sent to the model ([#1903](https://github.com/matthewyjiang/rho/issues/1903)) ([39a30ad](https://github.com/matthewyjiang/rho/commit/39a30ad2719f4882988b353a6100bf0f04904a34))
* **daemon:** add the rho daemon and the rho-protocol crate ([#1698](https://github.com/matthewyjiang/rho/issues/1698)) ([e0a4e35](https://github.com/matthewyjiang/rho/commit/e0a4e355e290f7ff3dc1bcb97c9779e8bba9a603))
* **daemon:** catch client and daemon version skew at the handshake, not at the first failed request ([#1784](https://github.com/matthewyjiang/rho/issues/1784)) ([6230b86](https://github.com/matthewyjiang/rho/commit/6230b867f3fc2f30cfa5cc17433bb2acee4810aa))
* **daemon:** clean up merged session worktrees and flag stale ones ([#1933](https://github.com/matthewyjiang/rho/issues/1933)) ([4a57c7d](https://github.com/matthewyjiang/rho/commit/4a57c7de30fc93facbc8269448efe1a1e76df8d9))
* **daemon:** configure a session's model, reasoning, fast mode and permission mode ([#1740](https://github.com/matthewyjiang/rho/issues/1740)) ([a6c9e39](https://github.com/matthewyjiang/rho/commit/a6c9e39f05d7905014456b44cf2ed62941dfbd7a))
* **daemon:** control daemon sessions over MCP and ensure startup ([#1763](https://github.com/matthewyjiang/rho/issues/1763)) ([83d23d1](https://github.com/matthewyjiang/rho/commit/83d23d1c3a073ffbaed8dc8f28ebb0c08f0a383e))
* **daemon:** declare a role and capabilities at the handshake and refuse answers from agents ([#1794](https://github.com/matthewyjiang/rho/issues/1794)) ([b270df3](https://github.com/matthewyjiang/rho/commit/b270df3c7d9cb24b3b4f444725b4f96a56fff740)), closes [#1780](https://github.com/matthewyjiang/rho/issues/1780)
* **daemon:** hold agent-started sessions to the grant they started with ([#1803](https://github.com/matthewyjiang/rho/issues/1803)) ([b2f1748](https://github.com/matthewyjiang/rho/commit/b2f17481cdaf7e92eade6ca0832796bf2b4787f3))
* **daemon:** hold each session's grant on its job record ([#1795](https://github.com/matthewyjiang/rho/issues/1795)) ([a49b012](https://github.com/matthewyjiang/rho/commit/a49b012fa86f0107268108300a3aeb97f3a6793e)), closes [#1780](https://github.com/matthewyjiang/rho/issues/1780)
* **daemon:** host sessions with a durable job registry ([#1712](https://github.com/matthewyjiang/rho/issues/1712)) ([985e9c8](https://github.com/matthewyjiang/rho/commit/985e9c8df206baab73e1c8338502a3238b3d71f8))
* **daemon:** host workflow runs as jobs on the shared agent budget ([#1718](https://github.com/matthewyjiang/rho/issues/1718)) ([34ad7b7](https://github.com/matthewyjiang/rho/commit/34ad7b74855a1901067c57c1b7a2384875e047f8))
* **daemon:** identify turns on the wire and make prompts safe to send again ([#1788](https://github.com/matthewyjiang/rho/issues/1788)) ([4d8e517](https://github.com/matthewyjiang/rho/commit/4d8e5170010b067fc6ae78d99633441d42f6c703))
* **daemon:** keep a session's prompt cache warm from the daemon ([#1878](https://github.com/matthewyjiang/rho/issues/1878)) ([79a6949](https://github.com/matthewyjiang/rho/commit/79a694966933646ab6d3010d855e9eda5f9381bd))
* **daemon:** keep the daemon in step with rho update and rho uninstall ([#1912](https://github.com/matthewyjiang/rho/issues/1912)) ([656b3f3](https://github.com/matthewyjiang/rho/commit/656b3f3b51b189d6ec7211774e8c0a8e0b42f0b9))
* **daemon:** let a session work in its own git worktree ([#1917](https://github.com/matthewyjiang/rho/issues/1917)) ([3431642](https://github.com/matthewyjiang/rho/commit/3431642ed7aa1135de8e533badc7bca6c2238069))
* **daemon:** let clients choose a session's agent, directories, MCP servers and parts ([#1747](https://github.com/matthewyjiang/rho/issues/1747)) ([0d4b7de](https://github.com/matthewyjiang/rho/commit/0d4b7de26d683303f9d6a59fb04ab73d801838a2))
* **daemon:** let rho attach follow a subagent job ([#1896](https://github.com/matthewyjiang/rho/issues/1896)) ([35105b2](https://github.com/matthewyjiang/rho/commit/35105b29444485ff93814b0937baf02058e6493b))
* **daemon:** list, rename and delete sessions ([#1741](https://github.com/matthewyjiang/rho/issues/1741)) ([464f770](https://github.com/matthewyjiang/rho/commit/464f77063a431de09d52d099853bf1c1a16b4a47))
* **daemon:** log in through the daemon and relay prompts to the client ([#1743](https://github.com/matthewyjiang/rho/issues/1743)) ([f158118](https://github.com/matthewyjiang/rho/commit/f15811877996ef7251a7afa754028677226d9d70))
* **daemon:** queue a settings change behind the turn instead of refusing it ([#1744](https://github.com/matthewyjiang/rho/issues/1744)) ([22c0292](https://github.com/matthewyjiang/rho/commit/22c029264d7cb6f65dda415b59a05ae469f6fbc3))
* **daemon:** queue a turn's approvals and questions for any client to answer ([#1719](https://github.com/matthewyjiang/rho/issues/1719)) ([5e7672a](https://github.com/matthewyjiang/rho/commit/5e7672ae142f548d4729c3c0d3d1000d2162da7c))
* **daemon:** read and change settings through the daemon protocol ([#1771](https://github.com/matthewyjiang/rho/issues/1771)) ([479d789](https://github.com/matthewyjiang/rho/commit/479d7890f4928cb097397f51912e086c99cd00a3))
* **daemon:** record who started a session and queue its turns on the shared budget ([fc949a1](https://github.com/matthewyjiang/rho/commit/fc949a1aa4a55d80bb9ef2b305eff09112145adf))
* **daemon:** reopen saved sessions on demand ([#1729](https://github.com/matthewyjiang/rho/issues/1729)) ([42d3c87](https://github.com/matthewyjiang/rho/commit/42d3c87838f761fbc90b204e7b44feaa715f321d))
* **daemon:** report the daemon's credential store and let clients choose it ([#1766](https://github.com/matthewyjiang/rho/issues/1766)) ([b9b0486](https://github.com/matthewyjiang/rho/commit/b9b0486556c39065f25d394976987263a82c72e0))
* **daemon:** restore a conversation tree node in a daemon session ([#1804](https://github.com/matthewyjiang/rho/issues/1804)) ([9f06eba](https://github.com/matthewyjiang/rho/commit/9f06ebab254ab8c987ba9a4b099f891aaa43a709))
* **daemon:** run /compact, /workflow and /agent: in daemon sessions ([#1748](https://github.com/matthewyjiang/rho/issues/1748)) ([d14c9fb](https://github.com/matthewyjiang/rho/commit/d14c9fb56ba0220afa899127ae5f28bd72be8040))
* **daemon:** run a session's CLI agents and workflow runs with its client's environment ([#1735](https://github.com/matthewyjiang/rho/issues/1735)) ([4265791](https://github.com/matthewyjiang/rho/commit/42657918ce8373ccf9492b7424f5eb73cbf6c660))
* **daemon:** run a session's commands with its client's environment ([#1733](https://github.com/matthewyjiang/rho/issues/1733)) ([0833ae2](https://github.com/matthewyjiang/rho/commit/0833ae2a9b11bf3979fb1cbbee47668c3090ed46))
* **daemon:** run a session's MCP servers, hooks, and OpenAPI sources with its client's environment ([#1734](https://github.com/matthewyjiang/rho/issues/1734)) ([20bf354](https://github.com/matthewyjiang/rho/commit/20bf354e3ef7ed09ef18f85ab0d328bc197f784c))
* **daemon:** run the daemon and the daemon TUI on Windows ([#1893](https://github.com/matthewyjiang/rho/issues/1893)) ([3777838](https://github.com/matthewyjiang/rho/commit/37778387e9180448b3098382d2c1bddaf39c049f))
* **daemon:** share one agent budget across sessions and list subagents as child jobs ([#1717](https://github.com/matthewyjiang/rho/issues/1717)) ([77dd2fe](https://github.com/matthewyjiang/rho/commit/77dd2fe6b533234e95da10a2085b15d28f9a410c))
* **daemon:** show cost and tool calls while a response streams ([#1809](https://github.com/matthewyjiang/rho/issues/1809)) ([b15909e](https://github.com/matthewyjiang/rho/commit/b15909ed44039d95b9f7ea94493fe42dd112ca84))
* **daemon:** sign the daemon in and out over its socket and let every session follow credential writes ([#1737](https://github.com/matthewyjiang/rho/issues/1737)) ([f0ad271](https://github.com/matthewyjiang/rho/commit/f0ad2710fa6ce454aba211e7fd90b667a36c6fbf))
* **daemon:** stream session events to attached clients ([#1715](https://github.com/matthewyjiang/rho/issues/1715)) ([198bfa3](https://github.com/matthewyjiang/rho/commit/198bfa3a334f5a71e5d41b88ea9828d060ea2bf5))
* **daemon:** take a subagent's question whether or not a turn runs ([#1838](https://github.com/matthewyjiang/rho/issues/1838)) ([abb432c](https://github.com/matthewyjiang/rho/commit/abb432c89cd835401344f879ee0f70cecc4dc115))
* **daemon:** take feedback with a plan approval ([#1811](https://github.com/matthewyjiang/rho/issues/1811)) ([8ce524b](https://github.com/matthewyjiang/rho/commit/8ce524bf87cf79cc31cdcf0d561bef28ce4e85f6))
* **daemon:** take workspace checkpoints and rewind in a daemon session ([#1806](https://github.com/matthewyjiang/rho/issues/1806)) ([115c679](https://github.com/matthewyjiang/rho/commit/115c67932c70c90da8f3b3e60b6e783eafe442b3))
* **daemon:** tell a client at rho/hello when the daemon is another build ([#1887](https://github.com/matthewyjiang/rho/issues/1887)) ([79b8be5](https://github.com/matthewyjiang/rho/commit/79b8be5b2babfd65d94d1e4fd0db5d2911c743b0))
* **daemon:** tell clients how each turn ended ([#1723](https://github.com/matthewyjiang/rho/issues/1723)) ([75362cc](https://github.com/matthewyjiang/rho/commit/75362cc94be9a02a036df8f79a2d56069a86d2be))
* **daemon:** time out questions with their fallback answers ([#1722](https://github.com/matthewyjiang/rho/issues/1722)) ([613022e](https://github.com/matthewyjiang/rho/commit/613022e50f479b0035fa27f0ab76f30b93569ec9))
* **daemon:** unload idle sessions ([#1731](https://github.com/matthewyjiang/rho/issues/1731)) ([b7eb939](https://github.com/matthewyjiang/rho/commit/b7eb939d449103c8a11861d5c66043209a7bc94c))
* **daemon:** upgrade the daemon in place without interrupting work ([#1772](https://github.com/matthewyjiang/rho/issues/1772)) ([341efb4](https://github.com/matthewyjiang/rho/commit/341efb4c13228546dbd593f0b73898fea8e3831f))
* **doctor:** show why the daemon is unreachable or behaves differently ([#1805](https://github.com/matthewyjiang/rho/issues/1805)) ([30ced27](https://github.com/matthewyjiang/rho/commit/30ced27e5d6bea93db49379a7c84c227de0d9ab0))
* drop computer use ([#1738](https://github.com/matthewyjiang/rho/issues/1738)) ([4ba3648](https://github.com/matthewyjiang/rho/commit/4ba36487c5d5c2ef3b37a89f212f8ce1e7effe35))
* **hooks:** show hooks the command the model requested when rtk rewrites it ([#1892](https://github.com/matthewyjiang/rho/issues/1892)) ([e2e01bd](https://github.com/matthewyjiang/rho/commit/e2e01bd35afe50f558efc5affae8fa0d8fe20712))
* **hooks:** tell hooks when a turn waits for input ([#1721](https://github.com/matthewyjiang/rho/issues/1721)) ([2258360](https://github.com/matthewyjiang/rho/commit/225836085c0b0cb3640b25fbd9fb64b00a83f422))
* **mcp:** add a read-only scope to rho mcp serve over stdio ([#1911](https://github.com/matthewyjiang/rho/issues/1911)) ([1b8cd4b](https://github.com/matthewyjiang/rho/commit/1b8cd4b0fa4d624191c7edbb1315cb7b89cc5e63))
* **mcp:** add authenticated HTTP access and service examples ([#1764](https://github.com/matthewyjiang/rho/issues/1764)) ([54817ee](https://github.com/matthewyjiang/rho/commit/54817ee3e5216a5ae36929f99ea4704d1956df08))
* **permissions:** show the auto classifier the command the model requested when rtk rewrites it ([#1894](https://github.com/matthewyjiang/rho/issues/1894)) ([01d5953](https://github.com/matthewyjiang/rho/commit/01d595314633578d1d24c674f1dcf016ba1ec68d))
* **protocol:** carry cost, titles, compactions and rich prompt content in daemon sessions ([#1739](https://github.com/matthewyjiang/rho/issues/1739)) ([bf6261a](https://github.com/matthewyjiang/rho/commit/bf6261afa7941b122c245c33fdfc5d6b338054f5))
* **protocol:** identify steers on the wire and let clients withdraw them ([#1756](https://github.com/matthewyjiang/rho/issues/1756)) ([23c8e7f](https://github.com/matthewyjiang/rho/commit/23c8e7f1ba2d525796a9a9f775b40414f587176c))
* **protocol:** let frontends find and start the daemon, and default unserved Service methods ([#1928](https://github.com/matthewyjiang/rho/issues/1928)) ([0dbff61](https://github.com/matthewyjiang/rho/commit/0dbff614fb19cc1a9d59573dbd3624050a5b7b03))
* **protocol:** list each job's title, model, cost, and open prompts ([#1897](https://github.com/matthewyjiang/rho/issues/1897)) ([934d5cd](https://github.com/matthewyjiang/rho/commit/934d5cd25bb9e003822a101d78dbb602b7e9e34d))
* **protocol:** send the TUI's tool cards on daemon tool calls ([#1754](https://github.com/matthewyjiang/rho/issues/1754)) ([9691ca0](https://github.com/matthewyjiang/rho/commit/9691ca0de58a11e6352e5e2a940ac9e56a48fc7e))
* **providers:** add Mistral AI, with one picker row per model across aliased ids ([#1742](https://github.com/matthewyjiang/rho/issues/1742)) ([ec15d36](https://github.com/matthewyjiang/rho/commit/ec15d36ec68ff07774de3edfb9bcc4134ea7db6c))
* **rho-protocol:** serve a connection's requests concurrently ([#1710](https://github.com/matthewyjiang/rho/issues/1710)) ([81f5e0a](https://github.com/matthewyjiang/rho/commit/81f5e0ae3e9d3875a3d862650f2aa7f0580312f5))
* **run:** start a run in the daemon with rho run --detach ([#1728](https://github.com/matthewyjiang/rho/issues/1728)) ([8f3e7e9](https://github.com/matthewyjiang/rho/commit/8f3e7e91af02a4ddde0010e285a5d76946319447))
* **run:** stream the daemon protocol's session events as JSONL ([#1724](https://github.com/matthewyjiang/rho/issues/1724)) ([f977570](https://github.com/matthewyjiang/rho/commit/f977570bf501ff8e4bbeb199c1bc08daeecbaa5d))
* **sdk:** add a session-control capability for acting on other sessions ([91b250f](https://github.com/matthewyjiang/rho/commit/91b250f5cfdabbd2e478d4067a299acb861de566))
* **session:** refuse a session another Rho process has open ([#1711](https://github.com/matthewyjiang/rho/issues/1711)) ([a58c159](https://github.com/matthewyjiang/rho/commit/a58c1592d76235f395206b896858314e611b4251))
* **sessions:** start and drive daemon sessions from the native sessions tool ([#1783](https://github.com/matthewyjiang/rho/issues/1783)) ([c4e7a22](https://github.com/matthewyjiang/rho/commit/c4e7a22002857db5ded6916123a3fbad05602987))
* **tui:** add /jobs to watch and control every daemon job across projects ([#1898](https://github.com/matthewyjiang/rho/issues/1898)) ([eec48a8](https://github.com/matthewyjiang/rho/commit/eec48a81a1837bb6f9b9c0b72214b0b7e5d7e185))
* **tui:** add directories, read context and hooks reports, and refresh models on the daemon ([#1798](https://github.com/matthewyjiang/rho/issues/1798)) ([679e0cf](https://github.com/matthewyjiang/rho/commit/679e0cfccede134dd20004b04c335e0ced50d369))
* **tui:** answer daemon input requests in the TUI's approval and questionnaire prompts ([#1759](https://github.com/matthewyjiang/rho/issues/1759)) ([dc12f8d](https://github.com/matthewyjiang/rho/commit/dc12f8dc96d7b39f3d0f45fc7a9829106a30745b))
* **tui:** apply --auth, --no-save and --agent on resume to a daemon session ([#1807](https://github.com/matthewyjiang/rho/issues/1807)) ([a08a0db](https://github.com/matthewyjiang/rho/commit/a08a0dbfa9fd7e3410868834b35b788acc34b3bc))
* **tui:** attach MCP resources and complete prompt arguments in daemon sessions ([#1870](https://github.com/matthewyjiang/rho/issues/1870)) ([f84b70a](https://github.com/matthewyjiang/rho/commit/f84b70ae747ce11f0f49804553fb0b0f1e28a7ad))
* **tui:** change settings and the advisor through the daemon ([#1774](https://github.com/matthewyjiang/rho/issues/1774)) ([b300283](https://github.com/matthewyjiang/rho/commit/b3002834ecbc166a01bd78e2eded3c9016201420))
* **tui:** configure daemon sessions, show their spend and title, and gate unported commands ([#1760](https://github.com/matthewyjiang/rho/issues/1760)) ([63174f2](https://github.com/matthewyjiang/rho/commit/63174f2f4e5fde6d9d84c740780b39a48cb9d891))
* **tui:** cycle reasoning from the statusline and join provider and model into one target ([#1840](https://github.com/matthewyjiang/rho/issues/1840)) ([067c1a2](https://github.com/matthewyjiang/rho/commit/067c1a24d8e8e094500924be6aff82f2b0fa2bf2))
* **tui:** delete sessions the daemon holds through the daemon ([#1816](https://github.com/matthewyjiang/rho/issues/1816)) ([e347fbd](https://github.com/matthewyjiang/rho/commit/e347fbd4961d334a64510538932154043bd824e9))
* **tui:** delete the in-process TUI ([#1904](https://github.com/matthewyjiang/rho/issues/1904)) ([933854d](https://github.com/matthewyjiang/rho/commit/933854d5d01916818312d8d9478ee33bedd296f9))
* **tui:** end a daemon turn while its subagents run and deliver their results when idle ([#1836](https://github.com/matthewyjiang/rho/issues/1836)) ([3134b0d](https://github.com/matthewyjiang/rho/commit/3134b0db9ac5769c0e375fef964d9a067b003f57))
* **tui:** end a daemon turn while its workflow runs and deliver the result when idle ([#1825](https://github.com/matthewyjiang/rho/issues/1825)) ([a8b7141](https://github.com/matthewyjiang/rho/commit/a8b7141dd498b5bd49bc0e065edf05c29bb1f48a))
* **tui:** follow daemon sessions in the TUI behind RHO_TUI_DAEMON ([#1757](https://github.com/matthewyjiang/rho/issues/1757)) ([b9564c6](https://github.com/matthewyjiang/rho/commit/b9564c69df91f4f6f519c60161e86b25410f0e84))
* **tui:** give a daemon session the in-process palette and command replies ([#1815](https://github.com/matthewyjiang/rho/issues/1815)) ([9504c3c](https://github.com/matthewyjiang/rho/commit/9504c3c41e2ac938d1a96d6974b86e431dbc7d1c))
* **tui:** make the daemon-backed TUI the default on Linux and macOS ([#1890](https://github.com/matthewyjiang/rho/issues/1890)) ([f27fe2b](https://github.com/matthewyjiang/rho/commit/f27fe2b28b4819835a710107313f6127b9082a8c))
* **tui:** match in-process model switching on a daemon session ([#1824](https://github.com/matthewyjiang/rho/issues/1824)) ([f90d2ef](https://github.com/matthewyjiang/rho/commit/f90d2ef1656de429d28adbcb15f045f3eb332001))
* **tui:** name models by their catalog display name in TUI chrome ([#1832](https://github.com/matthewyjiang/rho/issues/1832)) ([6fbc42c](https://github.com/matthewyjiang/rho/commit/6fbc42cfc33a2b532e8341efa122e13576b52a00))
* **tui:** reattach a lagged daemon session, and offer to resume a new one on exit ([#1761](https://github.com/matthewyjiang/rho/issues/1761)) ([7494490](https://github.com/matthewyjiang/rho/commit/7494490fc4243b05c0af5aa413ae33658d2c3d93))
* **tui:** reconnect the daemon-hosted TUI across a daemon restart ([#1773](https://github.com/matthewyjiang/rho/issues/1773)) ([fe80dd1](https://github.com/matthewyjiang/rho/commit/fe80dd128b3b3416fbb5461a4ddfa80b96ea64f2))
* **tui:** remember instructions, title sessions, and run shell commands in daemon sessions ([#1791](https://github.com/matthewyjiang/rho/issues/1791)) ([d783c45](https://github.com/matthewyjiang/rho/commit/d783c457f4cf21f90c87d0ab67db410fc3c5a5e1))
* **tui:** report a daemon session's MCP servers in /doctor ([#1839](https://github.com/matthewyjiang/rho/issues/1839)) ([bd21611](https://github.com/matthewyjiang/rho/commit/bd21611919af2a0ad3bdd66d8cf0572b851ec5aa))
* **tui:** run /goal and /mcp against the daemon session ([#1762](https://github.com/matthewyjiang/rho/issues/1762)) ([57d735e](https://github.com/matthewyjiang/rho/commit/57d735eea68b25ba5598900e37da84675690d6ee))
* **tui:** run first-run setup and the signed-out state on a daemon session ([#1813](https://github.com/matthewyjiang/rho/issues/1813)) ([3d6247d](https://github.com/matthewyjiang/rho/commit/3d6247d70c583a8af9bca2c70a7a1a8934d7340e))
* **tui:** run skill commands in a daemon session as in process ([#1828](https://github.com/matthewyjiang/rho/issues/1828)) ([c2a01a0](https://github.com/matthewyjiang/rho/commit/c2a01a0b4e5c1c984fea00642664a03cfba4212a))
* **tui:** run the workflow hub and codemode through the daemon ([#1802](https://github.com/matthewyjiang/rho/issues/1802)) ([396b417](https://github.com/matthewyjiang/rho/commit/396b4179614e8ecc443d6c8c02b06481d824efb0))
* **tui:** send prompts, steers and cancels to the followed daemon session ([#1758](https://github.com/matthewyjiang/rho/issues/1758)) ([19d2d0c](https://github.com/matthewyjiang/rho/commit/19d2d0c09b582902bc9f6656d4b33cea0afd2acc))
* **tui:** show a daemon session's processes on the rail and in peek ([#1853](https://github.com/matthewyjiang/rho/issues/1853)) ([e34c49f](https://github.com/matthewyjiang/rho/commit/e34c49fb2d08ea09ac354f266dcaa84112bfc2da))
* **tui:** show a daemon session's subagents, answer their questions, and manage agents ([#1793](https://github.com/matthewyjiang/rho/issues/1793)) ([42289f0](https://github.com/matthewyjiang/rho/commit/42289f07a00c491e9f5463ed7f9bb79c2579a7fd))
* **tui:** show compaction on a daemon session as in process ([#1834](https://github.com/matthewyjiang/rho/issues/1834)) ([6ecc03d](https://github.com/matthewyjiang/rho/commit/6ecc03d3cf05b690edf69d657e356bba03602707))
* **tui:** show MCP startup on a daemon session as in process ([#1830](https://github.com/matthewyjiang/rho/issues/1830)) ([7010775](https://github.com/matthewyjiang/rho/commit/701077590ced98941ff4ca791647037588583a89))
* **tui:** show provider retries, hosted searches and diagnostics on a daemon session ([#1829](https://github.com/matthewyjiang/rho/issues/1829)) ([6c21ce7](https://github.com/matthewyjiang/rho/commit/6c21ce72e4f89f43b9e0e44d9b475350fa1c4a57))
* **tui:** show the 2.x config upgrade once in the TUI, not just on stderr ([#1930](https://github.com/matthewyjiang/rho/issues/1930)) ([7c84788](https://github.com/matthewyjiang/rho/commit/7c84788b0054bfc092f1d1f7132043b2bdf30783))
* **tui:** show the daemon's health in /doctor ([#1932](https://github.com/matthewyjiang/rho/issues/1932)) ([a5af242](https://github.com/matthewyjiang/rho/commit/a5af242a66633447d24d1b2a002baed3367e7e13))
* **tui:** sign in and out through the daemon ([#1799](https://github.com/matthewyjiang/rho/issues/1799)) ([113c91a](https://github.com/matthewyjiang/rho/commit/113c91ab6644808a9c1ee17fe3969aa2739223c9))
* **tui:** start, resume, and pick daemon sessions without leaving the TUI ([#1792](https://github.com/matthewyjiang/rho/issues/1792)) ([fcc3225](https://github.com/matthewyjiang/rho/commit/fcc3225807d13f6dabdc0cc54dc54d9dc91696fa))
* **tui:** stream a daemon turn's text through the output stream ([#1814](https://github.com/matthewyjiang/rho/issues/1814)) ([5466f41](https://github.com/matthewyjiang/rho/commit/5466f41ae6505f9857c4de12ea295f859a82a888))


### Bug Fixes

* **acp:** close reattached v2 turns and keep approval titles ([#1922](https://github.com/matthewyjiang/rho/issues/1922)) ([e5a95a7](https://github.com/matthewyjiang/rho/commit/e5a95a7f104f6088c144f519d37db8108cf29c9b))
* **acp:** replay reasoning and tool calls as they streamed live ([#1694](https://github.com/matthewyjiang/rho/issues/1694)) ([009dde0](https://github.com/matthewyjiang/rho/commit/009dde0b193f117cdeca2025ce968bc416575899))
* **acp:** send 1-based tool-call location lines on v1 ([#1888](https://github.com/matthewyjiang/rho/issues/1888)) ([eb17abb](https://github.com/matthewyjiang/rho/commit/eb17abb9130057fe244c73b05483c4fef551a612))
* **acp:** title process calls with the command they start ([#1790](https://github.com/matthewyjiang/rho/issues/1790)) ([c35dd43](https://github.com/matthewyjiang/rho/commit/c35dd4371e604e326a4d17acd0e19f48872f3c10))
* **advisor:** advise on a custom provider added after a daemon session opened ([#1924](https://github.com/matthewyjiang/rho/issues/1924)) ([76871ac](https://github.com/matthewyjiang/rho/commit/76871ac3596ec67dab677d415318b213d9412fe1))
* **claude:** accept /usage panels without a 5-hour session window ([#1831](https://github.com/matthewyjiang/rho/issues/1831)) ([c45747b](https://github.com/matthewyjiang/rho/commit/c45747be8d62370f2f626e30d34a7448176036fe))
* **compaction:** collect follow-up metrics in daemon sessions ([#1914](https://github.com/matthewyjiang/rho/issues/1914)) ([8c5d9a9](https://github.com/matthewyjiang/rho/commit/8c5d9a950c776edd09e02234899da69595f67961))
* **config:** upgrade a 2.x config.toml at startup ([#1822](https://github.com/matthewyjiang/rho/issues/1822)) ([281a31a](https://github.com/matthewyjiang/rho/commit/281a31a5182fe1dd0326c473d175757160db7c59))
* **context:** stamp user messages with only their own send time ([#1906](https://github.com/matthewyjiang/rho/issues/1906)) ([bf3af5b](https://github.com/matthewyjiang/rho/commit/bf3af5b787bcd5753479e655303eaeb4d9c55645)), closes [#1903](https://github.com/matthewyjiang/rho/issues/1903)
* **daemon:** a new session fails clearly on a broken config.toml instead of opening on stale settings ([#1937](https://github.com/matthewyjiang/rho/issues/1937)) ([5b107f4](https://github.com/matthewyjiang/rho/commit/5b107f46c463d9af4c3f0e29988f1dc7ecef953b))
* **daemon:** a provider added or edited mid-session reaches loaded sessions at their next turn ([#1929](https://github.com/matthewyjiang/rho/issues/1929)) ([f2e5431](https://github.com/matthewyjiang/rho/commit/f2e5431f07d1779e822296d6c56b501637b1928e))
* **daemon:** clean up squash- and rebase-merged session worktrees ([#1938](https://github.com/matthewyjiang/rho/issues/1938)) ([1dcbe02](https://github.com/matthewyjiang/rho/commit/1dcbe024a1f7c8b7c364b0bc7806d2de5719959e))
* **daemon:** cut a loaded session's memory from 7x its content to under 2x ([#1935](https://github.com/matthewyjiang/rho/issues/1935)) ([8bb80e5](https://github.com/matthewyjiang/rho/commit/8bb80e58d46a849353fb1999d8f2093d9cb95adc)), closes [#1927](https://github.com/matthewyjiang/rho/issues/1927)
* **daemon:** discard unsaved sessions nobody follows ([#1753](https://github.com/matthewyjiang/rho/issues/1753)) ([8b676e3](https://github.com/matthewyjiang/rho/commit/8b676e3c886c50a3e77360aa8db8b7c19eccc07e))
* **daemon:** end an unreachable provider's turn with one clear error ([#1913](https://github.com/matthewyjiang/rho/issues/1913)) ([5a2f4c7](https://github.com/matthewyjiang/rho/commit/5a2f4c7503b79fece7646a8f3d29b7e10d801bbf))
* **daemon:** give each session its own client's credentials ([#1787](https://github.com/matthewyjiang/rho/issues/1787)) ([517312a](https://github.com/matthewyjiang/rho/commit/517312ac71c9571c8e9ae61afe718191e1ef0dd8)), closes [#1779](https://github.com/matthewyjiang/rho/issues/1779)
* **daemon:** have a goal wait for its subagents before it retries or is judged ([#1846](https://github.com/matthewyjiang/rho/issues/1846)) ([a40975e](https://github.com/matthewyjiang/rho/commit/a40975e8eab193a09d07010dd6c1157033ba2cda))
* **daemon:** keep each client's Ollama device key in its own directory ([#1800](https://github.com/matthewyjiang/rho/issues/1800)) ([ac7f33e](https://github.com/matthewyjiang/rho/commit/ac7f33e14a713f6415636afcb6393a444fbac2a3)), closes [#1789](https://github.com/matthewyjiang/rho/issues/1789)
* **daemon:** log an authentication failure's provider diagnostic ([#1833](https://github.com/matthewyjiang/rho/issues/1833)) ([baa62fd](https://github.com/matthewyjiang/rho/commit/baa62fdb3cc86a46917e784323e7079eb6a6979b))
* **daemon:** name the prompt file of a model switched to before the first prompt ([#1843](https://github.com/matthewyjiang/rho/issues/1843)) ([78d0d76](https://github.com/matthewyjiang/rho/commit/78d0d765d60cba35a5606285a6cc55fa1da63b21))
* **daemon:** open a session in auto without a classifier model, as in process ([#1866](https://github.com/matthewyjiang/rho/issues/1866)) ([3032a66](https://github.com/matthewyjiang/rho/commit/3032a6694b38e698e8a7510da8a5790802e039a3))
* **daemon:** rho attach shows idle after tree restore ([#1859](https://github.com/matthewyjiang/rho/issues/1859)) ([9476581](https://github.com/matthewyjiang/rho/commit/9476581a77dd93e9e5d1f69d41a432957b2d6c72))
* **daemon:** run fixture tool turns on a daemon session as in process ([#1849](https://github.com/matthewyjiang/rho/issues/1849)) ([c8b6745](https://github.com/matthewyjiang/rho/commit/c8b6745dfb8d0b3271a26ad6867760d5e2425464))
* **daemon:** show context and spend as soon as a client attaches ([#1920](https://github.com/matthewyjiang/rho/issues/1920)) ([92056f1](https://github.com/matthewyjiang/rho/commit/92056f1e958a450081fdf3429f41033f31adfc61))
* **daemon:** sign in on a headless machine and pick up logins from other processes ([#1801](https://github.com/matthewyjiang/rho/issues/1801)) ([f6bac82](https://github.com/matthewyjiang/rho/commit/f6bac828a1622d50dcf61434aae864e45eaafe1e))
* **daemon:** start a hub workflow without saving its prompt as a user message ([#1873](https://github.com/matthewyjiang/rho/issues/1873)) ([8029034](https://github.com/matthewyjiang/rho/commit/8029034d9655b18046a68f4cfa4b5c1b4c278761))
* **daemon:** start the daemon in its own systemd scope, not the client's unit ([#1925](https://github.com/matthewyjiang/rho/issues/1925)) ([5e3ea37](https://github.com/matthewyjiang/rho/commit/5e3ea37c1bd08abd78f5fe9067a52d76461c62aa))
* **daemon:** stop the first client deciding project trust for every session ([#1785](https://github.com/matthewyjiang/rho/issues/1785)) ([97864fa](https://github.com/matthewyjiang/rho/commit/97864fabac34cd49b3380470c5255d8cce2a68f5))
* **hooks:** let a rho a hook runs reach its session's daemon ([#1923](https://github.com/matthewyjiang/rho/issues/1923)) ([3ceb57b](https://github.com/matthewyjiang/rho/commit/3ceb57bc03a9d65b48b57c82397b62d13c23866e))
* **mcp:** hold delegated sessions to the caller's depth and token scope ([#1775](https://github.com/matthewyjiang/rho/issues/1775)) ([88e41d8](https://github.com/matthewyjiang/rho/commit/88e41d8b08f6f747a57b25af35713922457c0efe)), closes [#1765](https://github.com/matthewyjiang/rho/issues/1765)
* **mcp:** let a rho session's stdio rho mcp serve reach its own daemon ([#1918](https://github.com/matthewyjiang/rho/issues/1918)) ([fe33adf](https://github.com/matthewyjiang/rho/commit/fe33adf9a5a265a3c04d92c1670781eed2e90945))
* **providers:** title, compact, and advise on custom providers in daemon sessions ([#1919](https://github.com/matthewyjiang/rho/issues/1919)) ([9556dca](https://github.com/matthewyjiang/rho/commit/9556dca4e5ef8d052cde34f133c867c67b4584dc))
* **tui:** announce a daemon model switch whose request finishes before its options show ([#1885](https://github.com/matthewyjiang/rho/issues/1885)) ([ce12104](https://github.com/matthewyjiang/rho/commit/ce12104631edceb5d6615885e4097def818a16e1))
* **tui:** ask before a daemon session omits provider-native context ([#1858](https://github.com/matthewyjiang/rho/issues/1858)) ([cb55f47](https://github.com/matthewyjiang/rho/commit/cb55f4733474aa348313768710acc26f8babb603))
* **tui:** close the Bugbot follow-ups on the daemon TUI ([#1823](https://github.com/matthewyjiang/rho/issues/1823)) ([5b7d777](https://github.com/matthewyjiang/rho/commit/5b7d7771137772bb34cd9c45ca406530a4e10eba))
* **tui:** count a daemon session's finished subagent cost ([#1861](https://github.com/matthewyjiang/rho/issues/1861)) ([79bafed](https://github.com/matthewyjiang/rho/commit/79bafed4c9a065fde260f7f8be694ec34c119e69))
* **tui:** dismiss only a subagent's question on Esc while the parent turn runs ([#1860](https://github.com/matthewyjiang/rho/issues/1860)) ([7daaefc](https://github.com/matthewyjiang/rho/commit/7daaefc30b286831a360280accd3e3053216f2ee))
* **tui:** drop a daemon session's finished workflow row and the hub's prompt row ([#1854](https://github.com/matthewyjiang/rho/issues/1854)) ([78008df](https://github.com/matthewyjiang/rho/commit/78008dfaba9ffb516f1b4cdb731d306f793852bf))
* **tui:** hold a daemon session's Ollama device login until Enter ([#1827](https://github.com/matthewyjiang/rho/issues/1827)) ([8b60c7c](https://github.com/matthewyjiang/rho/commit/8b60c7c24f2e770bc9518d804b404e5fad074600))
* **tui:** hold a daemon session's prompt while its OpenAPI sources load ([#1864](https://github.com/matthewyjiang/rho/issues/1864)) ([9b48ec9](https://github.com/matthewyjiang/rho/commit/9b48ec95c7df26d9df3e1c0bae638b8b503893d5))
* **tui:** hold a daemon session's template prompt expanded while signed out ([#1857](https://github.com/matthewyjiang/rho/issues/1857)) ([c92b149](https://github.com/matthewyjiang/rho/commit/c92b149797c4435c605a764382728ef0622f20a3))
* **tui:** hold a daemon session's workflow launch and post-restart prompt while MCP connects ([#1847](https://github.com/matthewyjiang/rho/issues/1847)) ([f242719](https://github.com/matthewyjiang/rho/commit/f242719ff0f766703ca5f30eea430b724bccb904))
* **tui:** keep a daemon session's finished workflow row until its result is delivered ([#1872](https://github.com/matthewyjiang/rho/issues/1872)) ([73f6488](https://github.com/matthewyjiang/rho/commit/73f648815f961d67e1f113aec9614bc196f9ab1f))
* **tui:** keep a reopened daemon turn's clock and show waiting for provider ([#1886](https://github.com/matthewyjiang/rho/issues/1886)) ([762186b](https://github.com/matthewyjiang/rho/commit/762186b03dd38255deb0e224408f8555ed2aabee))
* **tui:** keep a reopened session's first frame and input clean on windows ([#1931](https://github.com/matthewyjiang/rho/issues/1931)) ([2e310bb](https://github.com/matthewyjiang/rho/commit/2e310bb3b7b41ac18323dbb5e24d57615d391974))
* **tui:** keep an approved plan's mode over a queued daemon mode change ([#1826](https://github.com/matthewyjiang/rho/issues/1826)) ([d5fb96f](https://github.com/matthewyjiang/rho/commit/d5fb96f5de45c0279bcc21b8257daf38f0f5b2fe))
* **tui:** line up each /info section's values in one column ([#1926](https://github.com/matthewyjiang/rho/issues/1926)) ([4cbe83c](https://github.com/matthewyjiang/rho/commit/4cbe83c81d3163684998da1acb97c86fa0c0a6ce))
* **tui:** match in process when a daemon session's compaction fails ([#1877](https://github.com/matthewyjiang/rho/issues/1877)) ([e2660a4](https://github.com/matthewyjiang/rho/commit/e2660a4aaa7c1ddc5c7fc099ce7d0969cda667a7))
* **tui:** offer compact, then continue on a daemon session's resume handoff ([#1871](https://github.com/matthewyjiang/rho/issues/1871)) ([578f815](https://github.com/matthewyjiang/rho/commit/578f8156368039d65b5f5a497a73f7c15157ed0f))
* **tui:** open each new daemon session on config.toml as it is then ([#1939](https://github.com/matthewyjiang/rho/issues/1939)) ([c57b83c](https://github.com/matthewyjiang/rho/commit/c57b83c7e533a33317ec5ccdecedd2c126d5346b))
* **tui:** open the daemon session with the first prompt, not at launch ([#1936](https://github.com/matthewyjiang/rho/issues/1936)) ([cfe3275](https://github.com/matthewyjiang/rho/commit/cfe3275cf9419426b713e43530428a89c1970526))
* **tui:** report a daemon questionnaire cancel as in process ([#1856](https://github.com/matthewyjiang/rho/issues/1856)) ([c8c8099](https://github.com/matthewyjiang/rho/commit/c8c8099abb795c31cfcbcd9e398cce164f12979f))
* **tui:** show a daemon turn reopened mid-reply as responding ([#1848](https://github.com/matthewyjiang/rho/issues/1848)) ([c5875ec](https://github.com/matthewyjiang/rho/commit/c5875ecc710d804a0a49013436b73adc22966320))
* **tui:** start a daemon session signed out when its saved model has no login ([#1835](https://github.com/matthewyjiang/rho/issues/1835)) ([71db3f0](https://github.com/matthewyjiang/rho/commit/71db3f078ae0f3e8b0ae5d18f15b57876ba54d11))
* **tui:** start a daemon session's MCP servers only in the daemon ([#1845](https://github.com/matthewyjiang/rho/issues/1845)) ([4243938](https://github.com/matthewyjiang/rho/commit/42439380a0b42558477a2738b10492d702e4428e))
* **tui:** stop long /info labels running into their values ([#1921](https://github.com/matthewyjiang/rho/issues/1921)) ([124cc08](https://github.com/matthewyjiang/rho/commit/124cc08f0238f8db220b24fe5cab2c7f1aa688b8))
* **workflow:** let the workflow tool read workspace sources on Windows ([#1910](https://github.com/matthewyjiang/rho/issues/1910)) ([56c12e8](https://github.com/matthewyjiang/rho/commit/56c12e859c4f7010fcd2ce1fc5aeb24f4604353e))
* **workflow:** reject command workflows at plan time where commands can't launch ([#1767](https://github.com/matthewyjiang/rho/issues/1767)) ([b3e20fa](https://github.com/matthewyjiang/rho/commit/b3e20fa7fecc0c59b5adbb55f0cb463e801c4fb1)), closes [#1736](https://github.com/matthewyjiang/rho/issues/1736)


### Performance Improvements

* **sdk:** share message content between copies of a history ([#1934](https://github.com/matthewyjiang/rho/issues/1934)) ([b80ba7c](https://github.com/matthewyjiang/rho/commit/b80ba7c3d42494d385ee677cf56c5f4ad120f14e))


### Code Refactoring

* **providers:** remove legacy login apis and fold model metadata sidecars ([#1689](https://github.com/matthewyjiang/rho/issues/1689)) ([01d1f07](https://github.com/matthewyjiang/rho/commit/01d1f079f002a47295267bcd8609977dee2d8d90)), closes [#1683](https://github.com/matthewyjiang/rho/issues/1683)
* **sdk:** carry tool outcomes and images on tool results ([#1693](https://github.com/matthewyjiang/rho/issues/1693)) ([4d52a30](https://github.com/matthewyjiang/rho/commit/4d52a30c553665ebeeeff61a70906afd206fc413))
* **sdk:** collapse provider turn methods and finish builder renames ([#1691](https://github.com/matthewyjiang/rho/issues/1691)) ([233e1f9](https://github.com/matthewyjiang/rho/commit/233e1f92be1f556c3469b9f8ead1447ee9e10579))
* **sdk:** mark host-facing sdk enums non-exhaustive ([#1687](https://github.com/matthewyjiang/rho/issues/1687)) ([3bac6f5](https://github.com/matthewyjiang/rho/commit/3bac6f582ffbce71703e66b176163d5017ff92ab))
* **sdk:** type compaction summaries, model context, and stop reasons in history ([#1695](https://github.com/matthewyjiang/rho/issues/1695)) ([4f44921](https://github.com/matthewyjiang/rho/commit/4f4492157a2835077b131a1ec719faae79f51bc8))
* **subagents:** name result.json session fields after the runtime, not claude ([#1688](https://github.com/matthewyjiang/rho/issues/1688)) ([03ed25f](https://github.com/matthewyjiang/rho/commit/03ed25f7d1cef193286b859e864c71834ff0df1b)), closes [#1678](https://github.com/matthewyjiang/rho/issues/1678)
* **tools:** remove the hashline edit format and the legacy rtk api ([#1692](https://github.com/matthewyjiang/rho/issues/1692)) ([f766d90](https://github.com/matthewyjiang/rho/commit/f766d90d46a7c614b2fc49d097e4ec9ef33680f1)), closes [#1684](https://github.com/matthewyjiang/rho/issues/1684)
* **workflow:** drop v1 workflow stores and compat json fields ([#1690](https://github.com/matthewyjiang/rho/issues/1690)) ([ed16175](https://github.com/matthewyjiang/rho/commit/ed16175813d47c6408ec4a2924ee4d1f43572220)), closes [#1677](https://github.com/matthewyjiang/rho/issues/1677)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 2.28.0 to 3.0.1
    * rho-protocol bumped from 0.1.0 to 1.0.0
</details>

<details><summary>rho-protocol: 1.0.0</summary>

## 1.0.0 (2026-11-13)


###   BREAKING CHANGES

* **daemon:** Message::User, Message::Assistant and Message::BoundaryInput hold Arc<[ContentBlock]> instead of Vec<ContentBlock>; Message::ToolResult holds Arc<ToolResult>; Message::EnrichedAssistant and Message::AbortedAssistant hold Arc instead of Box. Build them with `.into()` and change a shared payload with Arc::make_mut.

### Features

* **acp:** report and cancel background work over ACP v2 ([#1947](https://github.com/matthewyjiang/rho/issues/1947)) ([bc47bd7](https://github.com/matthewyjiang/rho/commit/bc47bd773a8431638aac618dc558c8b3bfb201fa))
* **acp:** run T3 Code child threads from daemon sessions ([#1750](https://github.com/matthewyjiang/rho/issues/1750)) ([33f646f](https://github.com/matthewyjiang/rho/commit/33f646fb9a069e0e9fdbeabadac440f3c67147e0))
* **attach:** answer, steer, and cancel a daemon session's turn from the terminal ([#1727](https://github.com/matthewyjiang/rho/issues/1727)) ([f4fba7e](https://github.com/matthewyjiang/rho/commit/f4fba7e259ce6e6c1ae0a2f64be1bd2733f94bb5))
* **attach:** follow a daemon session's turn over the protocol ([#1726](https://github.com/matthewyjiang/rho/issues/1726)) ([c3bf68e](https://github.com/matthewyjiang/rho/commit/c3bf68e72a65b0b0ddbf72f04ced8b9c2fa25e1e))
* **daemon:** add the rho daemon and the rho-protocol crate ([#1698](https://github.com/matthewyjiang/rho/issues/1698)) ([e0a4e35](https://github.com/matthewyjiang/rho/commit/e0a4e355e290f7ff3dc1bcb97c9779e8bba9a603))
* **daemon:** catch client and daemon version skew at the handshake, not at the first failed request ([#1784](https://github.com/matthewyjiang/rho/issues/1784)) ([6230b86](https://github.com/matthewyjiang/rho/commit/6230b867f3fc2f30cfa5cc17433bb2acee4810aa))
* **daemon:** clean up merged session worktrees and flag stale ones ([#1933](https://github.com/matthewyjiang/rho/issues/1933)) ([4a57c7d](https://github.com/matthewyjiang/rho/commit/4a57c7de30fc93facbc8269448efe1a1e76df8d9))
* **daemon:** configure a session's model, reasoning, fast mode and permission mode ([#1740](https://github.com/matthewyjiang/rho/issues/1740)) ([a6c9e39](https://github.com/matthewyjiang/rho/commit/a6c9e39f05d7905014456b44cf2ed62941dfbd7a))
* **daemon:** control daemon sessions over MCP and ensure startup ([#1763](https://github.com/matthewyjiang/rho/issues/1763)) ([83d23d1](https://github.com/matthewyjiang/rho/commit/83d23d1c3a073ffbaed8dc8f28ebb0c08f0a383e))
* **daemon:** declare a role and capabilities at the handshake and refuse answers from agents ([#1794](https://github.com/matthewyjiang/rho/issues/1794)) ([b270df3](https://github.com/matthewyjiang/rho/commit/b270df3c7d9cb24b3b4f444725b4f96a56fff740)), closes [#1780](https://github.com/matthewyjiang/rho/issues/1780)
* **daemon:** hold agent-started sessions to the grant they started with ([#1803](https://github.com/matthewyjiang/rho/issues/1803)) ([b2f1748](https://github.com/matthewyjiang/rho/commit/b2f17481cdaf7e92eade6ca0832796bf2b4787f3))
* **daemon:** hold each session's grant on its job record ([#1795](https://github.com/matthewyjiang/rho/issues/1795)) ([a49b012](https://github.com/matthewyjiang/rho/commit/a49b012fa86f0107268108300a3aeb97f3a6793e)), closes [#1780](https://github.com/matthewyjiang/rho/issues/1780)
* **daemon:** host sessions with a durable job registry ([#1712](https://github.com/matthewyjiang/rho/issues/1712)) ([985e9c8](https://github.com/matthewyjiang/rho/commit/985e9c8df206baab73e1c8338502a3238b3d71f8))
* **daemon:** host workflow runs as jobs on the shared agent budget ([#1718](https://github.com/matthewyjiang/rho/issues/1718)) ([34ad7b7](https://github.com/matthewyjiang/rho/commit/34ad7b74855a1901067c57c1b7a2384875e047f8))
* **daemon:** identify turns on the wire and make prompts safe to send again ([#1788](https://github.com/matthewyjiang/rho/issues/1788)) ([4d8e517](https://github.com/matthewyjiang/rho/commit/4d8e5170010b067fc6ae78d99633441d42f6c703))
* **daemon:** keep a session's prompt cache warm from the daemon ([#1878](https://github.com/matthewyjiang/rho/issues/1878)) ([79a6949](https://github.com/matthewyjiang/rho/commit/79a694966933646ab6d3010d855e9eda5f9381bd))
* **daemon:** keep the daemon in step with rho update and rho uninstall ([#1912](https://github.com/matthewyjiang/rho/issues/1912)) ([656b3f3](https://github.com/matthewyjiang/rho/commit/656b3f3b51b189d6ec7211774e8c0a8e0b42f0b9))
* **daemon:** let a session work in its own git worktree ([#1917](https://github.com/matthewyjiang/rho/issues/1917)) ([3431642](https://github.com/matthewyjiang/rho/commit/3431642ed7aa1135de8e533badc7bca6c2238069))
* **daemon:** let clients choose a session's agent, directories, MCP servers and parts ([#1747](https://github.com/matthewyjiang/rho/issues/1747)) ([0d4b7de](https://github.com/matthewyjiang/rho/commit/0d4b7de26d683303f9d6a59fb04ab73d801838a2))
* **daemon:** let rho attach follow a subagent job ([#1896](https://github.com/matthewyjiang/rho/issues/1896)) ([35105b2](https://github.com/matthewyjiang/rho/commit/35105b29444485ff93814b0937baf02058e6493b))
* **daemon:** list, rename and delete sessions ([#1741](https://github.com/matthewyjiang/rho/issues/1741)) ([464f770](https://github.com/matthewyjiang/rho/commit/464f77063a431de09d52d099853bf1c1a16b4a47))
* **daemon:** log in through the daemon and relay prompts to the client ([#1743](https://github.com/matthewyjiang/rho/issues/1743)) ([f158118](https://github.com/matthewyjiang/rho/commit/f15811877996ef7251a7afa754028677226d9d70))
* **daemon:** queue a settings change behind the turn instead of refusing it ([#1744](https://github.com/matthewyjiang/rho/issues/1744)) ([22c0292](https://github.com/matthewyjiang/rho/commit/22c029264d7cb6f65dda415b59a05ae469f6fbc3))
* **daemon:** queue a turn's approvals and questions for any client to answer ([#1719](https://github.com/matthewyjiang/rho/issues/1719)) ([5e7672a](https://github.com/matthewyjiang/rho/commit/5e7672ae142f548d4729c3c0d3d1000d2162da7c))
* **daemon:** read and change settings through the daemon protocol ([#1771](https://github.com/matthewyjiang/rho/issues/1771)) ([479d789](https://github.com/matthewyjiang/rho/commit/479d7890f4928cb097397f51912e086c99cd00a3))
* **daemon:** record who started a session and queue its turns on the shared budget ([fc949a1](https://github.com/matthewyjiang/rho/commit/fc949a1aa4a55d80bb9ef2b305eff09112145adf))
* **daemon:** reopen saved sessions on demand ([#1729](https://github.com/matthewyjiang/rho/issues/1729)) ([42d3c87](https://github.com/matthewyjiang/rho/commit/42d3c87838f761fbc90b204e7b44feaa715f321d))
* **daemon:** report the daemon's credential store and let clients choose it ([#1766](https://github.com/matthewyjiang/rho/issues/1766)) ([b9b0486](https://github.com/matthewyjiang/rho/commit/b9b0486556c39065f25d394976987263a82c72e0))
* **daemon:** restore a conversation tree node in a daemon session ([#1804](https://github.com/matthewyjiang/rho/issues/1804)) ([9f06eba](https://github.com/matthewyjiang/rho/commit/9f06ebab254ab8c987ba9a4b099f891aaa43a709))
* **daemon:** run /compact, /workflow and /agent: in daemon sessions ([#1748](https://github.com/matthewyjiang/rho/issues/1748)) ([d14c9fb](https://github.com/matthewyjiang/rho/commit/d14c9fb56ba0220afa899127ae5f28bd72be8040))
* **daemon:** run a session's CLI agents and workflow runs with its client's environment ([#1735](https://github.com/matthewyjiang/rho/issues/1735)) ([4265791](https://github.com/matthewyjiang/rho/commit/42657918ce8373ccf9492b7424f5eb73cbf6c660))
* **daemon:** run a session's commands with its client's environment ([#1733](https://github.com/matthewyjiang/rho/issues/1733)) ([0833ae2](https://github.com/matthewyjiang/rho/commit/0833ae2a9b11bf3979fb1cbbee47668c3090ed46))
* **daemon:** run a session's MCP servers, hooks, and OpenAPI sources with its client's environment ([#1734](https://github.com/matthewyjiang/rho/issues/1734)) ([20bf354](https://github.com/matthewyjiang/rho/commit/20bf354e3ef7ed09ef18f85ab0d328bc197f784c))
* **daemon:** run the daemon and the daemon TUI on Windows ([#1893](https://github.com/matthewyjiang/rho/issues/1893)) ([3777838](https://github.com/matthewyjiang/rho/commit/37778387e9180448b3098382d2c1bddaf39c049f))
* **daemon:** share one agent budget across sessions and list subagents as child jobs ([#1717](https://github.com/matthewyjiang/rho/issues/1717)) ([77dd2fe](https://github.com/matthewyjiang/rho/commit/77dd2fe6b533234e95da10a2085b15d28f9a410c))
* **daemon:** sign the daemon in and out over its socket and let every session follow credential writes ([#1737](https://github.com/matthewyjiang/rho/issues/1737)) ([f0ad271](https://github.com/matthewyjiang/rho/commit/f0ad2710fa6ce454aba211e7fd90b667a36c6fbf))
* **daemon:** stream session events to attached clients ([#1715](https://github.com/matthewyjiang/rho/issues/1715)) ([198bfa3](https://github.com/matthewyjiang/rho/commit/198bfa3a334f5a71e5d41b88ea9828d060ea2bf5))
* **daemon:** take a subagent's question whether or not a turn runs ([#1838](https://github.com/matthewyjiang/rho/issues/1838)) ([abb432c](https://github.com/matthewyjiang/rho/commit/abb432c89cd835401344f879ee0f70cecc4dc115))
* **daemon:** take feedback with a plan approval ([#1811](https://github.com/matthewyjiang/rho/issues/1811)) ([8ce524b](https://github.com/matthewyjiang/rho/commit/8ce524bf87cf79cc31cdcf0d561bef28ce4e85f6))
* **daemon:** take workspace checkpoints and rewind in a daemon session ([#1806](https://github.com/matthewyjiang/rho/issues/1806)) ([115c679](https://github.com/matthewyjiang/rho/commit/115c67932c70c90da8f3b3e60b6e783eafe442b3))
* **daemon:** tell a client at rho/hello when the daemon is another build ([#1887](https://github.com/matthewyjiang/rho/issues/1887)) ([79b8be5](https://github.com/matthewyjiang/rho/commit/79b8be5b2babfd65d94d1e4fd0db5d2911c743b0))
* **daemon:** tell clients how each turn ended ([#1723](https://github.com/matthewyjiang/rho/issues/1723)) ([75362cc](https://github.com/matthewyjiang/rho/commit/75362cc94be9a02a036df8f79a2d56069a86d2be))
* **daemon:** time out questions with their fallback answers ([#1722](https://github.com/matthewyjiang/rho/issues/1722)) ([613022e](https://github.com/matthewyjiang/rho/commit/613022e50f479b0035fa27f0ab76f30b93569ec9))
* **daemon:** upgrade the daemon in place without interrupting work ([#1772](https://github.com/matthewyjiang/rho/issues/1772)) ([341efb4](https://github.com/matthewyjiang/rho/commit/341efb4c13228546dbd593f0b73898fea8e3831f))
* **protocol:** carry cost, titles, compactions and rich prompt content in daemon sessions ([#1739](https://github.com/matthewyjiang/rho/issues/1739)) ([bf6261a](https://github.com/matthewyjiang/rho/commit/bf6261afa7941b122c245c33fdfc5d6b338054f5))
* **protocol:** identify steers on the wire and let clients withdraw them ([#1756](https://github.com/matthewyjiang/rho/issues/1756)) ([23c8e7f](https://github.com/matthewyjiang/rho/commit/23c8e7f1ba2d525796a9a9f775b40414f587176c))
* **protocol:** let frontends find and start the daemon, and default unserved Service methods ([#1928](https://github.com/matthewyjiang/rho/issues/1928)) ([0dbff61](https://github.com/matthewyjiang/rho/commit/0dbff614fb19cc1a9d59573dbd3624050a5b7b03))
* **protocol:** list each job's title, model, cost, and open prompts ([#1897](https://github.com/matthewyjiang/rho/issues/1897)) ([934d5cd](https://github.com/matthewyjiang/rho/commit/934d5cd25bb9e003822a101d78dbb602b7e9e34d))
* **protocol:** publish the protocol as JSON Schema with a compatibility check ([#1716](https://github.com/matthewyjiang/rho/issues/1716)) ([ba78abd](https://github.com/matthewyjiang/rho/commit/ba78abd1d2606ee7427077ba98f0e4b5e472da4c))
* **protocol:** send the TUI's tool cards on daemon tool calls ([#1754](https://github.com/matthewyjiang/rho/issues/1754)) ([9691ca0](https://github.com/matthewyjiang/rho/commit/9691ca0de58a11e6352e5e2a940ac9e56a48fc7e))
* **rho-protocol:** serve a connection's requests concurrently ([#1710](https://github.com/matthewyjiang/rho/issues/1710)) ([81f5e0a](https://github.com/matthewyjiang/rho/commit/81f5e0ae3e9d3875a3d862650f2aa7f0580312f5))
* **run:** start a run in the daemon with rho run --detach ([#1728](https://github.com/matthewyjiang/rho/issues/1728)) ([8f3e7e9](https://github.com/matthewyjiang/rho/commit/8f3e7e91af02a4ddde0010e285a5d76946319447))
* **sdk:** add a session-control capability for acting on other sessions ([91b250f](https://github.com/matthewyjiang/rho/commit/91b250f5cfdabbd2e478d4067a299acb861de566))
* **tui:** add directories, read context and hooks reports, and refresh models on the daemon ([#1798](https://github.com/matthewyjiang/rho/issues/1798)) ([679e0cf](https://github.com/matthewyjiang/rho/commit/679e0cfccede134dd20004b04c335e0ced50d369))
* **tui:** apply --auth, --no-save and --agent on resume to a daemon session ([#1807](https://github.com/matthewyjiang/rho/issues/1807)) ([a08a0db](https://github.com/matthewyjiang/rho/commit/a08a0dbfa9fd7e3410868834b35b788acc34b3bc))
* **tui:** attach MCP resources and complete prompt arguments in daemon sessions ([#1870](https://github.com/matthewyjiang/rho/issues/1870)) ([f84b70a](https://github.com/matthewyjiang/rho/commit/f84b70ae747ce11f0f49804553fb0b0f1e28a7ad))
* **tui:** delete sessions the daemon holds through the daemon ([#1816](https://github.com/matthewyjiang/rho/issues/1816)) ([e347fbd](https://github.com/matthewyjiang/rho/commit/e347fbd4961d334a64510538932154043bd824e9))
* **tui:** end a daemon turn while its subagents run and deliver their results when idle ([#1836](https://github.com/matthewyjiang/rho/issues/1836)) ([3134b0d](https://github.com/matthewyjiang/rho/commit/3134b0db9ac5769c0e375fef964d9a067b003f57))
* **tui:** end a daemon turn while its workflow runs and deliver the result when idle ([#1825](https://github.com/matthewyjiang/rho/issues/1825)) ([a8b7141](https://github.com/matthewyjiang/rho/commit/a8b7141dd498b5bd49bc0e065edf05c29bb1f48a))
* **tui:** remember instructions, title sessions, and run shell commands in daemon sessions ([#1791](https://github.com/matthewyjiang/rho/issues/1791)) ([d783c45](https://github.com/matthewyjiang/rho/commit/d783c457f4cf21f90c87d0ab67db410fc3c5a5e1))
* **tui:** run /goal and /mcp against the daemon session ([#1762](https://github.com/matthewyjiang/rho/issues/1762)) ([57d735e](https://github.com/matthewyjiang/rho/commit/57d735eea68b25ba5598900e37da84675690d6ee))
* **tui:** show a daemon session's processes on the rail and in peek ([#1853](https://github.com/matthewyjiang/rho/issues/1853)) ([e34c49f](https://github.com/matthewyjiang/rho/commit/e34c49fb2d08ea09ac354f266dcaa84112bfc2da))
* **tui:** show a daemon session's subagents, answer their questions, and manage agents ([#1793](https://github.com/matthewyjiang/rho/issues/1793)) ([42289f0](https://github.com/matthewyjiang/rho/commit/42289f07a00c491e9f5463ed7f9bb79c2579a7fd))
* **tui:** show compaction on a daemon session as in process ([#1834](https://github.com/matthewyjiang/rho/issues/1834)) ([6ecc03d](https://github.com/matthewyjiang/rho/commit/6ecc03d3cf05b690edf69d657e356bba03602707))
* **tui:** show provider retries, hosted searches and diagnostics on a daemon session ([#1829](https://github.com/matthewyjiang/rho/issues/1829)) ([6c21ce7](https://github.com/matthewyjiang/rho/commit/6c21ce72e4f89f43b9e0e44d9b475350fa1c4a57))


### Bug Fixes

* **daemon:** cut a loaded session's memory from 7x its content to under 2x ([#1935](https://github.com/matthewyjiang/rho/issues/1935)) ([8bb80e5](https://github.com/matthewyjiang/rho/commit/8bb80e58d46a849353fb1999d8f2093d9cb95adc)), closes [#1927](https://github.com/matthewyjiang/rho/issues/1927)
* **daemon:** end an unreachable provider's turn with one clear error ([#1913](https://github.com/matthewyjiang/rho/issues/1913)) ([5a2f4c7](https://github.com/matthewyjiang/rho/commit/5a2f4c7503b79fece7646a8f3d29b7e10d801bbf))
* **daemon:** give each session its own client's credentials ([#1787](https://github.com/matthewyjiang/rho/issues/1787)) ([517312a](https://github.com/matthewyjiang/rho/commit/517312ac71c9571c8e9ae61afe718191e1ef0dd8)), closes [#1779](https://github.com/matthewyjiang/rho/issues/1779)
* **daemon:** name the prompt file of a model switched to before the first prompt ([#1843](https://github.com/matthewyjiang/rho/issues/1843)) ([78d0d76](https://github.com/matthewyjiang/rho/commit/78d0d765d60cba35a5606285a6cc55fa1da63b21))
* **daemon:** start a hub workflow without saving its prompt as a user message ([#1873](https://github.com/matthewyjiang/rho/issues/1873)) ([8029034](https://github.com/matthewyjiang/rho/commit/8029034d9655b18046a68f4cfa4b5c1b4c278761))
* **daemon:** stop the first client deciding project trust for every session ([#1785](https://github.com/matthewyjiang/rho/issues/1785)) ([97864fa](https://github.com/matthewyjiang/rho/commit/97864fabac34cd49b3380470c5255d8cce2a68f5))
* **tui:** ask before a daemon session omits provider-native context ([#1858](https://github.com/matthewyjiang/rho/issues/1858)) ([cb55f47](https://github.com/matthewyjiang/rho/commit/cb55f4733474aa348313768710acc26f8babb603))
* **tui:** dismiss only a subagent's question on Esc while the parent turn runs ([#1860](https://github.com/matthewyjiang/rho/issues/1860)) ([7daaefc](https://github.com/matthewyjiang/rho/commit/7daaefc30b286831a360280accd3e3053216f2ee))
* **tui:** hold a daemon session's prompt while its OpenAPI sources load ([#1864](https://github.com/matthewyjiang/rho/issues/1864)) ([9b48ec9](https://github.com/matthewyjiang/rho/commit/9b48ec95c7df26d9df3e1c0bae638b8b503893d5))
* **tui:** keep a daemon session's finished workflow row until its result is delivered ([#1872](https://github.com/matthewyjiang/rho/issues/1872)) ([73f6488](https://github.com/matthewyjiang/rho/commit/73f648815f961d67e1f113aec9614bc196f9ab1f))
* **tui:** keep a reopened daemon turn's clock and show waiting for provider ([#1886](https://github.com/matthewyjiang/rho/issues/1886)) ([762186b](https://github.com/matthewyjiang/rho/commit/762186b03dd38255deb0e224408f8555ed2aabee))
* **tui:** match in process when a daemon session's compaction fails ([#1877](https://github.com/matthewyjiang/rho/issues/1877)) ([e2660a4](https://github.com/matthewyjiang/rho/commit/e2660a4aaa7c1ddc5c7fc099ce7d0969cda667a7))
* **tui:** start a daemon session signed out when its saved model has no login ([#1835](https://github.com/matthewyjiang/rho/issues/1835)) ([71db3f0](https://github.com/matthewyjiang/rho/commit/71db3f078ae0f3e8b0ae5d18f15b57876ba54d11))
</details>

<details><summary>rho-providers: 3.0.1</summary>

## [3.0.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v2.27.0...rho-providers-v3.0.1) (2026-11-13)


###   BREAKING CHANGES

* **sdk:** Message::User, Message::Assistant and Message::BoundaryInput hold Arc<[ContentBlock]> instead of Vec<ContentBlock>; Message::ToolResult holds Arc<ToolResult>; Message::EnrichedAssistant and Message::AbortedAssistant hold Arc instead of Box. Build them with `.into()` and change a shared payload with Arc::make_mut.
* **tui:** `RHO_TUI_DAEMON=0` no longer keeps a session in the TUI's own process; rho ignores the variable. MCP `sampling/createMessage`, which only the in-process session answered, is no longer answered.
* **sdk:** Message gains CompactionSummary, ModelContext, BoundaryInput, and ReasoningShift variants rendered through Message::wire(); ToolCall.execution replaces async marker blocks; ModelEvent gains ToolCallAccepted, Stopped, and ProviderContext with a ReplayScope; ProviderContextBlock.replay replaces the hard-coded Codex replay exception; the session snapshot schema is now 3. Saved 5.x history loads as typed entries.
* **sdk:** ToolResult carries status: ToolResultStatus and images instead of ok: bool; the tool image supplement message API is removed; ToolCompletion::Completed replaces Success and CompletedFailure; RunEvent::ToolStarted carries execution and replaces ToolDetached; ToolProgress uses new/with_metadata with getters. Saved 2.x sessions still load.
* **sdk:** ModelProvider has one send_turn_stream(request, ProviderTurnContext) and native_compact takes ModelRequestOptions; provider_steering_channel returns ProviderSteeringSender; StepStarted carries context_estimate; SteeringRetraction gains Delivered; InvalidConfiguration, Persistence, and Interrupted errors are struct variants with an optional source; host input and tool host builders use with_* setters and unsuffixed getters; AfterToolUsePayload carries capability and process.
* **sdk:** QuestionKind, DefaultSelection, HostInputSource, CompactorCancellationMode, HookSessionFailureKind, and ScriptedToolOutcome are now #[non_exhaustive]; exhaustive matches outside rho-sdk need a wildcard arm. Decision adapters should reject unknown question kinds with InvalidRequest::UnsupportedQuestionKind.
* **tools:** the hashline edit format and its `edit` tool are gone; `edit_tool = "hashline"` (global or per custom provider) now loads as `str_replace` with a migration warning. rho-agent-tools drops `EditFormat::Hashline`, the `hashline` module, `FileViewPolicy`/`FileViewStyle`, the public `rtk` module, the `Bash::new`/`PowerShell::new` RTK argument, and the `ToolCard` match_* fields, which move into the new `ToolBody::Search` variant alongside a new `ToolBody::Code` variant.
* **providers:** rho-providers removes run_codex_oauth_flow, run_xai_oauth_flow, run_openrouter_oauth_flow, the OAuth and Ollama device Browser error variants, start_ollama_device_login's open_browser argument, InteractiveUserAction, InteractiveLogin::user_action, accept_unconfirmed_login and the image_input lookup; start_interactive_login now takes BrowserAvailability (replacing start_interactive_login_with_availability), InteractiveLoginCompletion::Unconfirmed carries an UnconfirmedAuthentication handle to accept, and ModelMetadata gains image_input and reasoning_control. ModelMetadata, ProviderId, ProviderRuntime, ModelSelectionError, CatalogSdkAdapter and the new ReasoningControl are #[non_exhaustive], so downstream code must build ModelMetadata from Default and keep wildcard match arms.

### Features

* **context:** timestamp user messages and assistant text sent to the model ([#1903](https://github.com/matthewyjiang/rho/issues/1903)) ([39a30ad](https://github.com/matthewyjiang/rho/commit/39a30ad2719f4882988b353a6100bf0f04904a34))
* **daemon:** keep a session's prompt cache warm from the daemon ([#1878](https://github.com/matthewyjiang/rho/issues/1878)) ([79a6949](https://github.com/matthewyjiang/rho/commit/79a694966933646ab6d3010d855e9eda5f9381bd))
* **daemon:** keep the daemon in step with rho update and rho uninstall ([#1912](https://github.com/matthewyjiang/rho/issues/1912)) ([656b3f3](https://github.com/matthewyjiang/rho/commit/656b3f3b51b189d6ec7211774e8c0a8e0b42f0b9))
* **daemon:** log in through the daemon and relay prompts to the client ([#1743](https://github.com/matthewyjiang/rho/issues/1743)) ([f158118](https://github.com/matthewyjiang/rho/commit/f15811877996ef7251a7afa754028677226d9d70))
* **daemon:** report the daemon's credential store and let clients choose it ([#1766](https://github.com/matthewyjiang/rho/issues/1766)) ([b9b0486](https://github.com/matthewyjiang/rho/commit/b9b0486556c39065f25d394976987263a82c72e0))
* **daemon:** run a session's commands with its client's environment ([#1733](https://github.com/matthewyjiang/rho/issues/1733)) ([0833ae2](https://github.com/matthewyjiang/rho/commit/0833ae2a9b11bf3979fb1cbbee47668c3090ed46))
* **daemon:** run a session's MCP servers, hooks, and OpenAPI sources with its client's environment ([#1734](https://github.com/matthewyjiang/rho/issues/1734)) ([20bf354](https://github.com/matthewyjiang/rho/commit/20bf354e3ef7ed09ef18f85ab0d328bc197f784c))
* **daemon:** show cost and tool calls while a response streams ([#1809](https://github.com/matthewyjiang/rho/issues/1809)) ([b15909e](https://github.com/matthewyjiang/rho/commit/b15909ed44039d95b9f7ea94493fe42dd112ca84))
* **daemon:** take a subagent's question whether or not a turn runs ([#1838](https://github.com/matthewyjiang/rho/issues/1838)) ([abb432c](https://github.com/matthewyjiang/rho/commit/abb432c89cd835401344f879ee0f70cecc4dc115))
* drop computer use ([#1738](https://github.com/matthewyjiang/rho/issues/1738)) ([4ba3648](https://github.com/matthewyjiang/rho/commit/4ba36487c5d5c2ef3b37a89f212f8ce1e7effe35))
* **mcp:** add authenticated HTTP access and service examples ([#1764](https://github.com/matthewyjiang/rho/issues/1764)) ([54817ee](https://github.com/matthewyjiang/rho/commit/54817ee3e5216a5ae36929f99ea4704d1956df08))
* **providers:** add Mistral AI, with one picker row per model across aliased ids ([#1742](https://github.com/matthewyjiang/rho/issues/1742)) ([ec15d36](https://github.com/matthewyjiang/rho/commit/ec15d36ec68ff07774de3edfb9bcc4134ea7db6c))
* **tui:** add directories, read context and hooks reports, and refresh models on the daemon ([#1798](https://github.com/matthewyjiang/rho/issues/1798)) ([679e0cf](https://github.com/matthewyjiang/rho/commit/679e0cfccede134dd20004b04c335e0ced50d369))
* **tui:** change settings and the advisor through the daemon ([#1774](https://github.com/matthewyjiang/rho/issues/1774)) ([b300283](https://github.com/matthewyjiang/rho/commit/b3002834ecbc166a01bd78e2eded3c9016201420))
* **tui:** delete the in-process TUI ([#1904](https://github.com/matthewyjiang/rho/issues/1904)) ([933854d](https://github.com/matthewyjiang/rho/commit/933854d5d01916818312d8d9478ee33bedd296f9))
* **tui:** end a daemon turn while its subagents run and deliver their results when idle ([#1836](https://github.com/matthewyjiang/rho/issues/1836)) ([3134b0d](https://github.com/matthewyjiang/rho/commit/3134b0db9ac5769c0e375fef964d9a067b003f57))
* **tui:** make the daemon-backed TUI the default on Linux and macOS ([#1890](https://github.com/matthewyjiang/rho/issues/1890)) ([f27fe2b](https://github.com/matthewyjiang/rho/commit/f27fe2b28b4819835a710107313f6127b9082a8c))
* **tui:** name models by their catalog display name in TUI chrome ([#1832](https://github.com/matthewyjiang/rho/issues/1832)) ([6fbc42c](https://github.com/matthewyjiang/rho/commit/6fbc42cfc33a2b532e8341efa122e13576b52a00))
* **tui:** reattach a lagged daemon session, and offer to resume a new one on exit ([#1761](https://github.com/matthewyjiang/rho/issues/1761)) ([7494490](https://github.com/matthewyjiang/rho/commit/7494490fc4243b05c0af5aa413ae33658d2c3d93))
* **tui:** show provider retries, hosted searches and diagnostics on a daemon session ([#1829](https://github.com/matthewyjiang/rho/issues/1829)) ([6c21ce7](https://github.com/matthewyjiang/rho/commit/6c21ce72e4f89f43b9e0e44d9b475350fa1c4a57))


### Bug Fixes

* **compaction:** collect follow-up metrics in daemon sessions ([#1914](https://github.com/matthewyjiang/rho/issues/1914)) ([8c5d9a9](https://github.com/matthewyjiang/rho/commit/8c5d9a950c776edd09e02234899da69595f67961))
* **context:** stamp user messages with only their own send time ([#1906](https://github.com/matthewyjiang/rho/issues/1906)) ([bf3af5b](https://github.com/matthewyjiang/rho/commit/bf3af5b787bcd5753479e655303eaeb4d9c55645)), closes [#1903](https://github.com/matthewyjiang/rho/issues/1903)
* **daemon:** end an unreachable provider's turn with one clear error ([#1913](https://github.com/matthewyjiang/rho/issues/1913)) ([5a2f4c7](https://github.com/matthewyjiang/rho/commit/5a2f4c7503b79fece7646a8f3d29b7e10d801bbf))
* **daemon:** give each session its own client's credentials ([#1787](https://github.com/matthewyjiang/rho/issues/1787)) ([517312a](https://github.com/matthewyjiang/rho/commit/517312ac71c9571c8e9ae61afe718191e1ef0dd8)), closes [#1779](https://github.com/matthewyjiang/rho/issues/1779)
* **daemon:** keep each client's Ollama device key in its own directory ([#1800](https://github.com/matthewyjiang/rho/issues/1800)) ([ac7f33e](https://github.com/matthewyjiang/rho/commit/ac7f33e14a713f6415636afcb6393a444fbac2a3)), closes [#1789](https://github.com/matthewyjiang/rho/issues/1789)
* **tui:** dismiss only a subagent's question on Esc while the parent turn runs ([#1860](https://github.com/matthewyjiang/rho/issues/1860)) ([7daaefc](https://github.com/matthewyjiang/rho/commit/7daaefc30b286831a360280accd3e3053216f2ee))
* **tui:** start a daemon session signed out when its saved model has no login ([#1835](https://github.com/matthewyjiang/rho/issues/1835)) ([71db3f0](https://github.com/matthewyjiang/rho/commit/71db3f078ae0f3e8b0ae5d18f15b57876ba54d11))


### Performance Improvements

* **sdk:** share message content between copies of a history ([#1934](https://github.com/matthewyjiang/rho/issues/1934)) ([b80ba7c](https://github.com/matthewyjiang/rho/commit/b80ba7c3d42494d385ee677cf56c5f4ad120f14e))


### Code Refactoring

* **providers:** remove legacy login apis and fold model metadata sidecars ([#1689](https://github.com/matthewyjiang/rho/issues/1689)) ([01d1f07](https://github.com/matthewyjiang/rho/commit/01d1f079f002a47295267bcd8609977dee2d8d90)), closes [#1683](https://github.com/matthewyjiang/rho/issues/1683)
* **sdk:** carry tool outcomes and images on tool results ([#1693](https://github.com/matthewyjiang/rho/issues/1693)) ([4d52a30](https://github.com/matthewyjiang/rho/commit/4d52a30c553665ebeeeff61a70906afd206fc413))
* **sdk:** collapse provider turn methods and finish builder renames ([#1691](https://github.com/matthewyjiang/rho/issues/1691)) ([233e1f9](https://github.com/matthewyjiang/rho/commit/233e1f92be1f556c3469b9f8ead1447ee9e10579))
* **sdk:** mark host-facing sdk enums non-exhaustive ([#1687](https://github.com/matthewyjiang/rho/issues/1687)) ([3bac6f5](https://github.com/matthewyjiang/rho/commit/3bac6f582ffbce71703e66b176163d5017ff92ab))
* **sdk:** type compaction summaries, model context, and stop reasons in history ([#1695](https://github.com/matthewyjiang/rho/issues/1695)) ([4f44921](https://github.com/matthewyjiang/rho/commit/4f4492157a2835077b131a1ec719faae79f51bc8))
* **tools:** remove the hashline edit format and the legacy rtk api ([#1692](https://github.com/matthewyjiang/rho/issues/1692)) ([f766d90](https://github.com/matthewyjiang/rho/commit/f766d90d46a7c614b2fc49d097e4ec9ef33680f1)), closes [#1684](https://github.com/matthewyjiang/rho/issues/1684)
</details>

<details><summary>rho-sdk: 6.0.0</summary>

## [6.0.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v5.15.2...rho-sdk-v6.0.0) (2026-11-13)


###   BREAKING CHANGES

* **sdk:** Message::User, Message::Assistant and Message::BoundaryInput hold Arc<[ContentBlock]> instead of Vec<ContentBlock>; Message::ToolResult holds Arc<ToolResult>; Message::EnrichedAssistant and Message::AbortedAssistant hold Arc instead of Box. Build them with `.into()` and change a shared payload with Arc::make_mut.
* **hooks:** HookCapability::ExecuteProcess gains a requested_command field and HOOK_SCHEMA_VERSION is now 3.
* **sdk:** Message gains CompactionSummary, ModelContext, BoundaryInput, and ReasoningShift variants rendered through Message::wire(); ToolCall.execution replaces async marker blocks; ModelEvent gains ToolCallAccepted, Stopped, and ProviderContext with a ReplayScope; ProviderContextBlock.replay replaces the hard-coded Codex replay exception; the session snapshot schema is now 3. Saved 5.x history loads as typed entries.
* **sdk:** ToolResult carries status: ToolResultStatus and images instead of ok: bool; the tool image supplement message API is removed; ToolCompletion::Completed replaces Success and CompletedFailure; RunEvent::ToolStarted carries execution and replaces ToolDetached; ToolProgress uses new/with_metadata with getters. Saved 2.x sessions still load.
* **sdk:** ModelProvider has one send_turn_stream(request, ProviderTurnContext) and native_compact takes ModelRequestOptions; provider_steering_channel returns ProviderSteeringSender; StepStarted carries context_estimate; SteeringRetraction gains Delivered; InvalidConfiguration, Persistence, and Interrupted errors are struct variants with an optional source; host input and tool host builders use with_* setters and unsuffixed getters; AfterToolUsePayload carries capability and process.
* **sdk:** QuestionKind, DefaultSelection, HostInputSource, CompactorCancellationMode, HookSessionFailureKind, and ScriptedToolOutcome are now #[non_exhaustive]; exhaustive matches outside rho-sdk need a wildcard arm. Decision adapters should reject unknown question kinds with InvalidRequest::UnsupportedQuestionKind.

### Features

* **context:** timestamp user messages and assistant text sent to the model ([#1903](https://github.com/matthewyjiang/rho/issues/1903)) ([39a30ad](https://github.com/matthewyjiang/rho/commit/39a30ad2719f4882988b353a6100bf0f04904a34))
* **daemon:** configure a session's model, reasoning, fast mode and permission mode ([#1740](https://github.com/matthewyjiang/rho/issues/1740)) ([a6c9e39](https://github.com/matthewyjiang/rho/commit/a6c9e39f05d7905014456b44cf2ed62941dfbd7a))
* **daemon:** run a session's commands with its client's environment ([#1733](https://github.com/matthewyjiang/rho/issues/1733)) ([0833ae2](https://github.com/matthewyjiang/rho/commit/0833ae2a9b11bf3979fb1cbbee47668c3090ed46))
* **hooks:** show hooks the command the model requested when rtk rewrites it ([#1892](https://github.com/matthewyjiang/rho/issues/1892)) ([e2e01bd](https://github.com/matthewyjiang/rho/commit/e2e01bd35afe50f558efc5affae8fa0d8fe20712))
* **hooks:** tell hooks when a turn waits for input ([#1721](https://github.com/matthewyjiang/rho/issues/1721)) ([2258360](https://github.com/matthewyjiang/rho/commit/225836085c0b0cb3640b25fbd9fb64b00a83f422))
* **sdk:** add a session-control capability for acting on other sessions ([91b250f](https://github.com/matthewyjiang/rho/commit/91b250f5cfdabbd2e478d4067a299acb861de566))


### Bug Fixes

* **context:** stamp user messages with only their own send time ([#1906](https://github.com/matthewyjiang/rho/issues/1906)) ([bf3af5b](https://github.com/matthewyjiang/rho/commit/bf3af5b787bcd5753479e655303eaeb4d9c55645)), closes [#1903](https://github.com/matthewyjiang/rho/issues/1903)


### Performance Improvements

* **sdk:** share message content between copies of a history ([#1934](https://github.com/matthewyjiang/rho/issues/1934)) ([b80ba7c](https://github.com/matthewyjiang/rho/commit/b80ba7c3d42494d385ee677cf56c5f4ad120f14e))


### Code Refactoring

* **sdk:** carry tool outcomes and images on tool results ([#1693](https://github.com/matthewyjiang/rho/issues/1693)) ([4d52a30](https://github.com/matthewyjiang/rho/commit/4d52a30c553665ebeeeff61a70906afd206fc413))
* **sdk:** collapse provider turn methods and finish builder renames ([#1691](https://github.com/matthewyjiang/rho/issues/1691)) ([233e1f9](https://github.com/matthewyjiang/rho/commit/233e1f92be1f556c3469b9f8ead1447ee9e10579))
* **sdk:** mark host-facing sdk enums non-exhaustive ([#1687](https://github.com/matthewyjiang/rho/issues/1687)) ([3bac6f5](https://github.com/matthewyjiang/rho/commit/3bac6f582ffbce71703e66b176163d5017ff92ab))
* **sdk:** type compaction summaries, model context, and stop reasons in history ([#1695](https://github.com/matthewyjiang/rho/issues/1695)) ([4f44921](https://github.com/matthewyjiang/rho/commit/4f4492157a2835077b131a1ec719faae79f51bc8))
</details>

---
This PR was generated with [Release Please](https://github.com/googleapis/release-please). See [documentation](https://github.com/googleapis/release-please#release-please).