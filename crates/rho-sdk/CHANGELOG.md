# Changelog

## [5.2.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v5.2.0...rho-sdk-v5.2.1) (2026-09-05)


### Bug Fixes

* **runtime:** reconcile background notifications before completion ([#1166](https://github.com/matthewyjiang/rho/issues/1166)) ([b5630f6](https://github.com/matthewyjiang/rho/commit/b5630f671233519a3036608534f52d8ea8a51ac7))


### Performance Improvements

* **runtime:** speed up session replay and streamed rendering ([#1163](https://github.com/matthewyjiang/rho/issues/1163)) ([242460c](https://github.com/matthewyjiang/rho/commit/242460c7d7e84c7c11d3503d0c027ca409284922))

## [5.2.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v5.1.2...rho-sdk-v5.2.0) (2026-09-04)


### Features

* **openai:** forward mid-turn steering over the Codex websocket on gpt-6-astra ([#1150](https://github.com/matthewyjiang/rho/issues/1150)) ([3628bda](https://github.com/matthewyjiang/rho/commit/3628bda95035a5b13da18f1bd42404622d1a39cd))
* **openai:** send reasoning changes as configuration_update on gpt-6-astra ([#1149](https://github.com/matthewyjiang/rho/issues/1149)) ([7928476](https://github.com/matthewyjiang/rho/commit/792847602525f27743240ae9aa194c4ce9f50324))
* **rho-sdk:** run async tool calls as detached jobs ([#1151](https://github.com/matthewyjiang/rho/issues/1151)) ([ed4c276](https://github.com/matthewyjiang/rho/commit/ed4c276011a5d649b258b9583774db65ba44c4e2))

## [5.1.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v5.1.1...rho-sdk-v5.1.2) (2026-09-03)


### Bug Fixes

* **compaction:** /compact always shrinks context on large-window models ([#1132](https://github.com/matthewyjiang/rho/issues/1132)) ([d7522b6](https://github.com/matthewyjiang/rho/commit/d7522b68b9c224e11092ad16d95717156b6b7452))

## [5.1.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v5.1.0...rho-sdk-v5.1.1) (2026-08-30)


### Bug Fixes

* **sdk:** stop burst-replayed proxy streams from inflating tok/s ([#1103](https://github.com/matthewyjiang/rho/issues/1103)) ([d4e88ae](https://github.com/matthewyjiang/rho/commit/d4e88aea5d61195eb267faa49877dad11f650c29))

## [5.1.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v5.0.0...rho-sdk-v5.1.0) (2026-08-30)


### Features

* **sdk:** put the requested capability on after_tool_use ([#1090](https://github.com/matthewyjiang/rho/issues/1090)) ([90f1a0e](https://github.com/matthewyjiang/rho/commit/90f1a0ec781930f8ed98b6f7b80953ffc04a7587))
* **tui:** add /side overlay for frozen-context asides ([#1094](https://github.com/matthewyjiang/rho/issues/1094)) ([3ebbdf9](https://github.com/matthewyjiang/rho/commit/3ebbdf951f01c6a32e827d4b819bdd9441c76684))

## [5.0.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v4.2.0...rho-sdk-v5.0.0) (2026-08-27)


### ⚠ BREAKING CHANGES

* rho-sdk removes RunEvent::ProviderActivity and the PROVIDER_ACTIVITY_* constants, collapses ProviderStreamResetReason retryable variants into RetryableFailure { kind, retry_after }, removes ModelCallMetrics::output_tokens_per_second, adds revision to RunEvent::Failed, folds ContextEstimated into StepStarted, and replaces the stringly generation-output-token carriers with a typed ModelCallMetrics::generation_output_tokens metric and ModelEvent::GenerationOutputTokens variant. rho-providers adds ProviderId::OpenAiCompatible for config-defined hosts, folds CustomProviderOptions into CustomProviderSpec (removing the _with_lookup/ _with_options funnels), makes providers() include custom hosts (removing visible_providers), removes deprecated registry API-base re-exports and ProviderRuntime::same_family, and drops the CredentialStore parameter from OpenAiProvider::new_with_identity. rho-tools removes the edit_file tool-name alias, the write_file frontmatter alias, and the EditToolKind type alias.

### Code Refactoring

* pay down all NEXT_MAJOR debt for the next major, net -986 lines ([#1062](https://github.com/matthewyjiang/rho/issues/1062)) ([0e17bdb](https://github.com/matthewyjiang/rho/commit/0e17bdb5e9fa21440fffbbe00171830ac0d1d3c2))

## [4.2.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v4.1.1...rho-sdk-v4.2.0) (2026-08-21)


### Features

* **acp:** let hosts set reasoning through thought_level ([#1015](https://github.com/matthewyjiang/rho/issues/1015)) ([5cda8e8](https://github.com/matthewyjiang/rho/commit/5cda8e8308682fa13c9d6001ec12989f0d030bb4))

## [4.1.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v4.1.0...rho-sdk-v4.1.1) (2026-08-20)


### Bug Fixes

* **tui:** stop openai-compatible cost jumping on submit ([#1008](https://github.com/matthewyjiang/rho/issues/1008)) ([51f881f](https://github.com/matthewyjiang/rho/commit/51f881f68d663b1d15a0b280f0fef76cbcf9a8ba))

## [4.1.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v4.0.1...rho-sdk-v4.1.0) (2026-08-17)


### Features

* **xai:** let grok generate and edit images in the conversation ([#967](https://github.com/matthewyjiang/rho/issues/967)) ([47c03c1](https://github.com/matthewyjiang/rho/commit/47c03c1d2b13864d32a69b27c21da4db54572e0e))

## [4.0.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v4.0.0...rho-sdk-v4.0.1) (2026-08-15)


### Bug Fixes

* **session:** v1 upgrades no longer write an unloadable transcript ([#939](https://github.com/matthewyjiang/rho/issues/939)) ([1dc1002](https://github.com/matthewyjiang/rho/commit/1dc1002c5c60ddc2a2889a12d92ead905cd4b6d3))

## [4.0.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v3.1.0...rho-sdk-v4.0.0) (2026-08-12)


### ⚠ BREAKING CHANGES

* **hooks:** `AuthorizationDenialKind` and `ApprovalAuditDecision` gain a hook variant. Both are `#[non_exhaustive]`, so only exhaustive matches written before this change need updating.
* **xai:** ModelEvent::WebSearch and RunEvent::WebSearch now carry a name field so hosts can distinguish web_search from x_search.

### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))
* **hooks:** add typed lifecycle hooks ([#668](https://github.com/matthewyjiang/rho/issues/668)) ([4a69c3d](https://github.com/matthewyjiang/rho/commit/4a69c3dfbc2136a8c23dff515909e23886b8651f))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **permission:** rename Auto to Bypass and add classifier Auto ([#870](https://github.com/matthewyjiang/rho/issues/870)) ([3192daa](https://github.com/matthewyjiang/rho/commit/3192daa713f7202f44727ec4acb83d0c646d1286))
* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))
* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))
* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **questionnaire:** add focused default selection ([#530](https://github.com/matthewyjiang/rho/issues/530)) ([47d1853](https://github.com/matthewyjiang/rho/commit/47d185377a68881c96db18ee186b3173215e39ee))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))
* **sdk:** add embeddable Rho runtime ([#262](https://github.com/matthewyjiang/rho/issues/262)) ([6fdac81](https://github.com/matthewyjiang/rho/commit/6fdac81b2a2d68331b72ecf768ad7631dada9d72))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **sessions:** add conversation tree navigation ([#474](https://github.com/matthewyjiang/rho/issues/474)) ([8abb138](https://github.com/matthewyjiang/rho/commit/8abb1387a0c96dcf3142166d27b4db108d1c5181))
* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tui:** add retractable pending input ([#334](https://github.com/matthewyjiang/rho/issues/334)) ([5f293a2](https://github.com/matthewyjiang/rho/commit/5f293a2221e0dcd5457eccbc8675eed2463d878e))
* **tui:** redesign questionnaire with tabbed question layout ([#369](https://github.com/matthewyjiang/rho/issues/369)) ([a90135a](https://github.com/matthewyjiang/rho/commit/a90135a494409cfc1c99ffd2226bee9075788d41))
* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))
* **tui:** show detailed activity stages ([#403](https://github.com/matthewyjiang/rho/issues/403)) ([267a47b](https://github.com/matthewyjiang/rho/commit/267a47bd3894f7a6ed64c82d98920a2c2d585e91))
* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))
* **usage:** add durable request ledger ([#381](https://github.com/matthewyjiang/rho/issues/381)) ([0502b99](https://github.com/matthewyjiang/rho/commit/0502b9987be74a8922f675ab941eadb23bc88b12))
* **workflow:** add deterministic DAG workflows ([#680](https://github.com/matthewyjiang/rho/issues/680)) ([77f36f9](https://github.com/matthewyjiang/rho/commit/77f36f9cc6992f78ab59ac481a1f2dc4e00350a0))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **metrics:** include reasoning latency in output rate ([#632](https://github.com/matthewyjiang/rho/issues/632)) ([7f7fa39](https://github.com/matthewyjiang/rho/commit/7f7fa39d88e3032a4433a105e07a785989406944))
* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))
* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))
* **providers:** show bounded error diagnostics ([#344](https://github.com/matthewyjiang/rho/issues/344)) ([e3fc489](https://github.com/matthewyjiang/rho/commit/e3fc48984590d34e19238e157e2479fa3c9d0d20))
* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))
* **sdk:** commit turn history on provider failure ([#739](https://github.com/matthewyjiang/rho/issues/739)) ([b4158ab](https://github.com/matthewyjiang/rho/commit/b4158ab1a58974dd29e43f6970dbe2fd08f714b5))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))
* **sdk:** serialize identical concurrent approval requests to prompt once ([#469](https://github.com/matthewyjiang/rho/issues/469)) ([45b2b0a](https://github.com/matthewyjiang/rho/commit/45b2b0a34fd3a2482f515a4a4e613fd18ffa103f))
* **sdk:** stabilize compaction release benchmark ([#561](https://github.com/matthewyjiang/rho/issues/561)) ([8364edc](https://github.com/matthewyjiang/rho/commit/8364edc7f8d1acb3967a061b0da02fd4a102a787))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **subagents:** run multi-agent batches in parallel ([#544](https://github.com/matthewyjiang/rho/issues/544)) ([7dd6706](https://github.com/matthewyjiang/rho/commit/7dd6706f6aade0a45d70336a42d259b4a3c12a4f))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** open approval prompts at the start and default to deny ([#636](https://github.com/matthewyjiang/rho/issues/636)) ([59efc07](https://github.com/matthewyjiang/rho/commit/59efc07b26bf67597ebbe05551cd22f3affedc96))
* **tui:** report generation token throughput ([#803](https://github.com/matthewyjiang/rho/issues/803)) ([4772f68](https://github.com/matthewyjiang/rho/commit/4772f68ccad3fc1edf65aa666d9a49f74bfab960))
* **tui:** report herdr blocked during questionnaire ([#500](https://github.com/matthewyjiang/rho/issues/500)) ([1dcdbe9](https://github.com/matthewyjiang/rho/commit/1dcdbe9e86bb61cf0cd55a705c28941d9dfcb241))
* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))
* **tui:** stabilize live stream tool cards and markdown previews ([#595](https://github.com/matthewyjiang/rho/issues/595)) ([752794f](https://github.com/matthewyjiang/rho/commit/752794f407d65533e97924d6e89ceeba443886c0))


### Performance Improvements

* optimize orchestration and session hot paths ([#603](https://github.com/matthewyjiang/rho/issues/603)) ([62aa8f5](https://github.com/matthewyjiang/rho/commit/62aa8f50358fc82f1e6bef5bf0d2348fc6c0aaac))
* reduce hot-path allocations and redundant I/O ([#280](https://github.com/matthewyjiang/rho/issues/280)) ([c18e582](https://github.com/matthewyjiang/rho/commit/c18e5823156254dccf59080864e775990c1b89cb))
* **sdk:** avoid duplicate history clone during compaction ([#276](https://github.com/matthewyjiang/rho/issues/276)) ([79e3926](https://github.com/matthewyjiang/rho/commit/79e3926f2de855860c3418baa67a3dc78aa20870))

## [3.0.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v2.1.0...rho-sdk-v3.0.0) (2026-08-11)


### ⚠ BREAKING CHANGES

* **hooks:** `AuthorizationDenialKind` and `ApprovalAuditDecision` gain a hook variant. Both are `#[non_exhaustive]`, so only exhaustive matches written before this change need updating.
* **xai:** ModelEvent::WebSearch and RunEvent::WebSearch now carry a name field so hosts can distinguish web_search from x_search.

### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))
* **hooks:** add typed lifecycle hooks ([#668](https://github.com/matthewyjiang/rho/issues/668)) ([4a69c3d](https://github.com/matthewyjiang/rho/commit/4a69c3dfbc2136a8c23dff515909e23886b8651f))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))
* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))
* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **questionnaire:** add focused default selection ([#530](https://github.com/matthewyjiang/rho/issues/530)) ([47d1853](https://github.com/matthewyjiang/rho/commit/47d185377a68881c96db18ee186b3173215e39ee))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))
* **sdk:** add embeddable Rho runtime ([#262](https://github.com/matthewyjiang/rho/issues/262)) ([6fdac81](https://github.com/matthewyjiang/rho/commit/6fdac81b2a2d68331b72ecf768ad7631dada9d72))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **sessions:** add conversation tree navigation ([#474](https://github.com/matthewyjiang/rho/issues/474)) ([8abb138](https://github.com/matthewyjiang/rho/commit/8abb1387a0c96dcf3142166d27b4db108d1c5181))
* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tui:** add retractable pending input ([#334](https://github.com/matthewyjiang/rho/issues/334)) ([5f293a2](https://github.com/matthewyjiang/rho/commit/5f293a2221e0dcd5457eccbc8675eed2463d878e))
* **tui:** redesign questionnaire with tabbed question layout ([#369](https://github.com/matthewyjiang/rho/issues/369)) ([a90135a](https://github.com/matthewyjiang/rho/commit/a90135a494409cfc1c99ffd2226bee9075788d41))
* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))
* **tui:** show detailed activity stages ([#403](https://github.com/matthewyjiang/rho/issues/403)) ([267a47b](https://github.com/matthewyjiang/rho/commit/267a47bd3894f7a6ed64c82d98920a2c2d585e91))
* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))
* **usage:** add durable request ledger ([#381](https://github.com/matthewyjiang/rho/issues/381)) ([0502b99](https://github.com/matthewyjiang/rho/commit/0502b9987be74a8922f675ab941eadb23bc88b12))
* **workflow:** add deterministic DAG workflows ([#680](https://github.com/matthewyjiang/rho/issues/680)) ([77f36f9](https://github.com/matthewyjiang/rho/commit/77f36f9cc6992f78ab59ac481a1f2dc4e00350a0))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **metrics:** include reasoning latency in output rate ([#632](https://github.com/matthewyjiang/rho/issues/632)) ([7f7fa39](https://github.com/matthewyjiang/rho/commit/7f7fa39d88e3032a4433a105e07a785989406944))
* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))
* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))
* **providers:** show bounded error diagnostics ([#344](https://github.com/matthewyjiang/rho/issues/344)) ([e3fc489](https://github.com/matthewyjiang/rho/commit/e3fc48984590d34e19238e157e2479fa3c9d0d20))
* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))
* **sdk:** commit turn history on provider failure ([#739](https://github.com/matthewyjiang/rho/issues/739)) ([b4158ab](https://github.com/matthewyjiang/rho/commit/b4158ab1a58974dd29e43f6970dbe2fd08f714b5))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))
* **sdk:** serialize identical concurrent approval requests to prompt once ([#469](https://github.com/matthewyjiang/rho/issues/469)) ([45b2b0a](https://github.com/matthewyjiang/rho/commit/45b2b0a34fd3a2482f515a4a4e613fd18ffa103f))
* **sdk:** stabilize compaction release benchmark ([#561](https://github.com/matthewyjiang/rho/issues/561)) ([8364edc](https://github.com/matthewyjiang/rho/commit/8364edc7f8d1acb3967a061b0da02fd4a102a787))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **subagents:** run multi-agent batches in parallel ([#544](https://github.com/matthewyjiang/rho/issues/544)) ([7dd6706](https://github.com/matthewyjiang/rho/commit/7dd6706f6aade0a45d70336a42d259b4a3c12a4f))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** open approval prompts at the start and default to deny ([#636](https://github.com/matthewyjiang/rho/issues/636)) ([59efc07](https://github.com/matthewyjiang/rho/commit/59efc07b26bf67597ebbe05551cd22f3affedc96))
* **tui:** report generation token throughput ([#803](https://github.com/matthewyjiang/rho/issues/803)) ([4772f68](https://github.com/matthewyjiang/rho/commit/4772f68ccad3fc1edf65aa666d9a49f74bfab960))
* **tui:** report herdr blocked during questionnaire ([#500](https://github.com/matthewyjiang/rho/issues/500)) ([1dcdbe9](https://github.com/matthewyjiang/rho/commit/1dcdbe9e86bb61cf0cd55a705c28941d9dfcb241))
* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))
* **tui:** stabilize live stream tool cards and markdown previews ([#595](https://github.com/matthewyjiang/rho/issues/595)) ([752794f](https://github.com/matthewyjiang/rho/commit/752794f407d65533e97924d6e89ceeba443886c0))


### Performance Improvements

* optimize orchestration and session hot paths ([#603](https://github.com/matthewyjiang/rho/issues/603)) ([62aa8f5](https://github.com/matthewyjiang/rho/commit/62aa8f50358fc82f1e6bef5bf0d2348fc6c0aaac))
* reduce hot-path allocations and redundant I/O ([#280](https://github.com/matthewyjiang/rho/issues/280)) ([c18e582](https://github.com/matthewyjiang/rho/commit/c18e5823156254dccf59080864e775990c1b89cb))
* **sdk:** avoid duplicate history clone during compaction ([#276](https://github.com/matthewyjiang/rho/issues/276)) ([79e3926](https://github.com/matthewyjiang/rho/commit/79e3926f2de855860c3418baa67a3dc78aa20870))

## [2.0.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.18.0...rho-sdk-v2.0.0) (2026-08-10)


### ⚠ BREAKING CHANGES

* **hooks:** `AuthorizationDenialKind` and `ApprovalAuditDecision` gain a hook variant. Both are `#[non_exhaustive]`, so only exhaustive matches written before this change need updating.
* **xai:** ModelEvent::WebSearch and RunEvent::WebSearch now carry a name field so hosts can distinguish web_search from x_search.

### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))
* **hooks:** add typed lifecycle hooks ([#668](https://github.com/matthewyjiang/rho/issues/668)) ([4a69c3d](https://github.com/matthewyjiang/rho/commit/4a69c3dfbc2136a8c23dff515909e23886b8651f))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))
* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))
* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **questionnaire:** add focused default selection ([#530](https://github.com/matthewyjiang/rho/issues/530)) ([47d1853](https://github.com/matthewyjiang/rho/commit/47d185377a68881c96db18ee186b3173215e39ee))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))
* **sdk:** add embeddable Rho runtime ([#262](https://github.com/matthewyjiang/rho/issues/262)) ([6fdac81](https://github.com/matthewyjiang/rho/commit/6fdac81b2a2d68331b72ecf768ad7631dada9d72))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **sessions:** add conversation tree navigation ([#474](https://github.com/matthewyjiang/rho/issues/474)) ([8abb138](https://github.com/matthewyjiang/rho/commit/8abb1387a0c96dcf3142166d27b4db108d1c5181))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tui:** add retractable pending input ([#334](https://github.com/matthewyjiang/rho/issues/334)) ([5f293a2](https://github.com/matthewyjiang/rho/commit/5f293a2221e0dcd5457eccbc8675eed2463d878e))
* **tui:** redesign questionnaire with tabbed question layout ([#369](https://github.com/matthewyjiang/rho/issues/369)) ([a90135a](https://github.com/matthewyjiang/rho/commit/a90135a494409cfc1c99ffd2226bee9075788d41))
* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))
* **tui:** show detailed activity stages ([#403](https://github.com/matthewyjiang/rho/issues/403)) ([267a47b](https://github.com/matthewyjiang/rho/commit/267a47bd3894f7a6ed64c82d98920a2c2d585e91))
* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))
* **usage:** add durable request ledger ([#381](https://github.com/matthewyjiang/rho/issues/381)) ([0502b99](https://github.com/matthewyjiang/rho/commit/0502b9987be74a8922f675ab941eadb23bc88b12))
* **workflow:** add deterministic DAG workflows ([#680](https://github.com/matthewyjiang/rho/issues/680)) ([77f36f9](https://github.com/matthewyjiang/rho/commit/77f36f9cc6992f78ab59ac481a1f2dc4e00350a0))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **metrics:** include reasoning latency in output rate ([#632](https://github.com/matthewyjiang/rho/issues/632)) ([7f7fa39](https://github.com/matthewyjiang/rho/commit/7f7fa39d88e3032a4433a105e07a785989406944))
* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))
* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))
* **providers:** show bounded error diagnostics ([#344](https://github.com/matthewyjiang/rho/issues/344)) ([e3fc489](https://github.com/matthewyjiang/rho/commit/e3fc48984590d34e19238e157e2479fa3c9d0d20))
* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))
* **sdk:** commit turn history on provider failure ([#739](https://github.com/matthewyjiang/rho/issues/739)) ([b4158ab](https://github.com/matthewyjiang/rho/commit/b4158ab1a58974dd29e43f6970dbe2fd08f714b5))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))
* **sdk:** serialize identical concurrent approval requests to prompt once ([#469](https://github.com/matthewyjiang/rho/issues/469)) ([45b2b0a](https://github.com/matthewyjiang/rho/commit/45b2b0a34fd3a2482f515a4a4e613fd18ffa103f))
* **sdk:** stabilize compaction release benchmark ([#561](https://github.com/matthewyjiang/rho/issues/561)) ([8364edc](https://github.com/matthewyjiang/rho/commit/8364edc7f8d1acb3967a061b0da02fd4a102a787))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **subagents:** run multi-agent batches in parallel ([#544](https://github.com/matthewyjiang/rho/issues/544)) ([7dd6706](https://github.com/matthewyjiang/rho/commit/7dd6706f6aade0a45d70336a42d259b4a3c12a4f))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** open approval prompts at the start and default to deny ([#636](https://github.com/matthewyjiang/rho/issues/636)) ([59efc07](https://github.com/matthewyjiang/rho/commit/59efc07b26bf67597ebbe05551cd22f3affedc96))
* **tui:** report generation token throughput ([#803](https://github.com/matthewyjiang/rho/issues/803)) ([4772f68](https://github.com/matthewyjiang/rho/commit/4772f68ccad3fc1edf65aa666d9a49f74bfab960))
* **tui:** report herdr blocked during questionnaire ([#500](https://github.com/matthewyjiang/rho/issues/500)) ([1dcdbe9](https://github.com/matthewyjiang/rho/commit/1dcdbe9e86bb61cf0cd55a705c28941d9dfcb241))
* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))
* **tui:** stabilize live stream tool cards and markdown previews ([#595](https://github.com/matthewyjiang/rho/issues/595)) ([752794f](https://github.com/matthewyjiang/rho/commit/752794f407d65533e97924d6e89ceeba443886c0))


### Performance Improvements

* optimize orchestration and session hot paths ([#603](https://github.com/matthewyjiang/rho/issues/603)) ([62aa8f5](https://github.com/matthewyjiang/rho/commit/62aa8f50358fc82f1e6bef5bf0d2348fc6c0aaac))
* reduce hot-path allocations and redundant I/O ([#280](https://github.com/matthewyjiang/rho/issues/280)) ([c18e582](https://github.com/matthewyjiang/rho/commit/c18e5823156254dccf59080864e775990c1b89cb))
* **sdk:** avoid duplicate history clone during compaction ([#276](https://github.com/matthewyjiang/rho/issues/276)) ([79e3926](https://github.com/matthewyjiang/rho/commit/79e3926f2de855860c3418baa67a3dc78aa20870))

## [1.17.3](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.17.2...rho-sdk-v1.17.3) (2026-08-08)


### Bug Fixes

* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))

## [1.17.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.17.1...rho-sdk-v1.17.2) (2026-08-07)


### Bug Fixes

* **tui:** report generation token throughput ([#803](https://github.com/matthewyjiang/rho/issues/803)) ([4772f68](https://github.com/matthewyjiang/rho/commit/4772f68ccad3fc1edf65aa666d9a49f74bfab960))

## [1.17.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.17.0...rho-sdk-v1.17.1) (2026-08-07)


### Bug Fixes

* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))

## [1.17.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.16.0...rho-sdk-v1.17.0) (2026-08-06)


### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))

## [1.16.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.15.2...rho-sdk-v1.16.0) (2026-08-04)


### Features

* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))

## [1.15.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.15.1...rho-sdk-v1.15.2) (2026-08-04)


### Bug Fixes

* **sdk:** commit turn history on provider failure ([#739](https://github.com/matthewyjiang/rho/issues/739)) ([b4158ab](https://github.com/matthewyjiang/rho/commit/b4158ab1a58974dd29e43f6970dbe2fd08f714b5))

## [1.15.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.15.0...rho-sdk-v1.15.1) (2026-08-03)


### Bug Fixes

* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))

## [1.15.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.14.0...rho-sdk-v1.15.0) (2026-08-02)


### Features

* **workflow:** add deterministic DAG workflows ([#680](https://github.com/matthewyjiang/rho/issues/680)) ([77f36f9](https://github.com/matthewyjiang/rho/commit/77f36f9cc6992f78ab59ac481a1f2dc4e00350a0))

## [1.14.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.13.1...rho-sdk-v1.14.0) (2026-07-31)


### Features

* **hooks:** add typed lifecycle hooks ([#668](https://github.com/matthewyjiang/rho/issues/668)) ([4a69c3d](https://github.com/matthewyjiang/rho/commit/4a69c3dfbc2136a8c23dff515909e23886b8651f))

## [1.13.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.13.0...rho-sdk-v1.13.1) (2026-07-30)


### Bug Fixes

* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))

## [1.13.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.12.2...rho-sdk-v1.13.0) (2026-07-30)


### Features

* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))

## [1.12.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.12.1...rho-sdk-v1.12.2) (2026-07-29)


### Bug Fixes

* **tui:** open approval prompts at the start and default to deny ([#636](https://github.com/matthewyjiang/rho/issues/636)) ([59efc07](https://github.com/matthewyjiang/rho/commit/59efc07b26bf67597ebbe05551cd22f3affedc96))

## [1.12.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.12.0...rho-sdk-v1.12.1) (2026-07-28)


### Bug Fixes

* **metrics:** include reasoning latency in output rate ([#632](https://github.com/matthewyjiang/rho/issues/632)) ([7f7fa39](https://github.com/matthewyjiang/rho/commit/7f7fa39d88e3032a4433a105e07a785989406944))

## [1.12.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.11.0...rho-sdk-v1.12.0) (2026-07-28)


### Features

* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))

## [1.11.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.10.3...rho-sdk-v1.11.0) (2026-07-28)


### Features

* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))


### Performance Improvements

* optimize orchestration and session hot paths ([#603](https://github.com/matthewyjiang/rho/issues/603)) ([62aa8f5](https://github.com/matthewyjiang/rho/commit/62aa8f50358fc82f1e6bef5bf0d2348fc6c0aaac))

## [1.10.3](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.10.2...rho-sdk-v1.10.3) (2026-07-27)


### Bug Fixes

* **tui:** stabilize live stream tool cards and markdown previews ([#595](https://github.com/matthewyjiang/rho/issues/595)) ([752794f](https://github.com/matthewyjiang/rho/commit/752794f407d65533e97924d6e89ceeba443886c0))

## [1.10.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.10.1...rho-sdk-v1.10.2) (2026-07-27)


### Bug Fixes

* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))

## [1.10.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.10.0...rho-sdk-v1.10.1) (2026-07-26)


### Bug Fixes

* **sdk:** stabilize compaction release benchmark ([#561](https://github.com/matthewyjiang/rho/issues/561)) ([8364edc](https://github.com/matthewyjiang/rho/commit/8364edc7f8d1acb3967a061b0da02fd4a102a787))

## [1.10.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.9.0...rho-sdk-v1.10.0) (2026-07-25)


### Features

* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))


### Bug Fixes

* **subagents:** run multi-agent batches in parallel ([#544](https://github.com/matthewyjiang/rho/issues/544)) ([7dd6706](https://github.com/matthewyjiang/rho/commit/7dd6706f6aade0a45d70336a42d259b4a3c12a4f))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))

## [1.9.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.8.0...rho-sdk-v1.9.0) (2026-07-24)


### Features

* **questionnaire:** add focused default selection ([#530](https://github.com/matthewyjiang/rho/issues/530)) ([47d1853](https://github.com/matthewyjiang/rho/commit/47d185377a68881c96db18ee186b3173215e39ee))

## [1.8.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.7.2...rho-sdk-v1.8.0) (2026-07-23)


### Features

* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))


### Bug Fixes

* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))

## [1.7.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.7.1...rho-sdk-v1.7.2) (2026-07-22)


### Bug Fixes

* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** report herdr blocked during questionnaire ([#500](https://github.com/matthewyjiang/rho/issues/500)) ([1dcdbe9](https://github.com/matthewyjiang/rho/commit/1dcdbe9e86bb61cf0cd55a705c28941d9dfcb241))

## [1.7.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.7.0...rho-sdk-v1.7.1) (2026-07-22)


### Bug Fixes

* **sdk:** serialize identical concurrent approval requests to prompt once ([#469](https://github.com/matthewyjiang/rho/issues/469)) ([45b2b0a](https://github.com/matthewyjiang/rho/commit/45b2b0a34fd3a2482f515a4a4e613fd18ffa103f))

## [1.7.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.6.0...rho-sdk-v1.7.0) (2026-07-22)


### Features

* **sessions:** add conversation tree navigation ([#474](https://github.com/matthewyjiang/rho/issues/474)) ([8abb138](https://github.com/matthewyjiang/rho/commit/8abb1387a0c96dcf3142166d27b4db108d1c5181))

## [1.6.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.5.0...rho-sdk-v1.6.0) (2026-07-21)


### Features

* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))


### Bug Fixes

* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))

## [1.5.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.4.0...rho-sdk-v1.5.0) (2026-07-20)


### Features

* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))

## [1.4.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.3.0...rho-sdk-v1.4.0) (2026-07-20)


### Features

* **tui:** show detailed activity stages ([#403](https://github.com/matthewyjiang/rho/issues/403)) ([267a47b](https://github.com/matthewyjiang/rho/commit/267a47bd3894f7a6ed64c82d98920a2c2d585e91))


### Bug Fixes

* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))

## [1.3.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.2.0...rho-sdk-v1.3.0) (2026-07-18)


### Features

* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))

## [1.2.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.1.0...rho-sdk-v1.2.0) (2026-07-17)


### Features

* **tui:** redesign questionnaire with tabbed question layout ([#369](https://github.com/matthewyjiang/rho/issues/369)) ([a90135a](https://github.com/matthewyjiang/rho/commit/a90135a494409cfc1c99ffd2226bee9075788d41))
* **usage:** add durable request ledger ([#381](https://github.com/matthewyjiang/rho/issues/381)) ([0502b99](https://github.com/matthewyjiang/rho/commit/0502b9987be74a8922f675ab941eadb23bc88b12))

## [1.1.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.0.2...rho-sdk-v1.1.0) (2026-07-16)


### Features

* **tui:** add retractable pending input ([#334](https://github.com/matthewyjiang/rho/issues/334)) ([5f293a2](https://github.com/matthewyjiang/rho/commit/5f293a2221e0dcd5457eccbc8675eed2463d878e))


### Bug Fixes

* **providers:** show bounded error diagnostics ([#344](https://github.com/matthewyjiang/rho/issues/344)) ([e3fc489](https://github.com/matthewyjiang/rho/commit/e3fc48984590d34e19238e157e2479fa3c9d0d20))

## [1.0.2](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.0.1...rho-sdk-v1.0.2) (2026-07-15)


### Performance Improvements

* reduce hot-path allocations and redundant I/O ([#280](https://github.com/matthewyjiang/rho/issues/280)) ([c18e582](https://github.com/matthewyjiang/rho/commit/c18e5823156254dccf59080864e775990c1b89cb))

## [1.0.1](https://github.com/matthewyjiang/rho/compare/rho-sdk-v1.0.0...rho-sdk-v1.0.1) (2026-07-15)


### Performance Improvements

* **sdk:** avoid duplicate history clone during compaction ([#276](https://github.com/matthewyjiang/rho/issues/276)) ([79e3926](https://github.com/matthewyjiang/rho/commit/79e3926f2de855860c3418baa67a3dc78aa20870))

## [1.0.0](https://github.com/matthewyjiang/rho/compare/rho-sdk-v0.1.0...rho-sdk-v1.0.0) (2026-07-15)


### Features

* **sdk:** add embeddable Rho runtime ([#262](https://github.com/matthewyjiang/rho/issues/262)) ([6fdac81](https://github.com/matthewyjiang/rho/commit/6fdac81b2a2d68331b72ecf768ad7631dada9d72))
