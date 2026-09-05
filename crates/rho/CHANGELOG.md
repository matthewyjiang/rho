# Changelog

## [2.8.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.7.0...rho-coding-agent-v2.8.0) (2026-09-05)


### Features

* **tui:** show task-first agent message previews ([#1162](https://github.com/matthewyjiang/rho/issues/1162)) ([ead555d](https://github.com/matthewyjiang/rho/commit/ead555dd976b62f7fd1d7ec1350da97c44c3dbcb))


### Bug Fixes

* **agents:** resolve host auth for the target provider ([#1170](https://github.com/matthewyjiang/rho/issues/1170)) ([66bd205](https://github.com/matthewyjiang/rho/commit/66bd2051fec65bb8ce4ee9b6add1814dbfc4e2fa))
* **runtime:** reconcile background notifications before completion ([#1166](https://github.com/matthewyjiang/rho/issues/1166)) ([b5630f6](https://github.com/matthewyjiang/rho/commit/b5630f671233519a3036608534f52d8ea8a51ac7))
* **subagents:** deliver completions after starting a new session ([#1171](https://github.com/matthewyjiang/rho/issues/1171)) ([0304725](https://github.com/matthewyjiang/rho/commit/03047257ff640c2d6bb9907853c8129a9cb74685))
* **tui:** apply reasoning visibility to the whole transcript ([#1168](https://github.com/matthewyjiang/rho/issues/1168)) ([f4c3708](https://github.com/matthewyjiang/rho/commit/f4c3708c9328de1af575775ffc3ee8a22a11bb1b))
* **tui:** render side chat like the main transcript ([#1172](https://github.com/matthewyjiang/rho/issues/1172)) ([827c746](https://github.com/matthewyjiang/rho/commit/827c74612d220341eab75cf50d2215746f06a2e0))


### Performance Improvements

* **runtime:** speed up session replay and streamed rendering ([#1163](https://github.com/matthewyjiang/rho/issues/1163)) ([242460c](https://github.com/matthewyjiang/rho/commit/242460c7d7e84c7c11d3503d0c027ca409284922))
* **subagents:** stop routine notices from waking parents ([#1167](https://github.com/matthewyjiang/rho/issues/1167)) ([1990b25](https://github.com/matthewyjiang/rho/commit/1990b25802e21b24abd494c87b07eed3cdf32d16))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 5.3.0 to 5.2.1
    * rho-providers bumped from 2.3.0 to 2.4.0

## [2.7.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.6.0...rho-coding-agent-v2.7.0) (2026-09-04)


### Features

* **config:** configure auto edit tools for custom providers ([#1156](https://github.com/matthewyjiang/rho/issues/1156)) ([9a6f320](https://github.com/matthewyjiang/rho/commit/9a6f320192ce58824e521fa54b98d6c0cdf6e9ee))
* **openai:** forward mid-turn steering over the Codex websocket on gpt-6-astra ([#1150](https://github.com/matthewyjiang/rho/issues/1150)) ([3628bda](https://github.com/matthewyjiang/rho/commit/3628bda95035a5b13da18f1bd42404622d1a39cd))
* **openai:** wire async tool calling for gpt-6-astra ([#1152](https://github.com/matthewyjiang/rho/issues/1152)) ([9fcc988](https://github.com/matthewyjiang/rho/commit/9fcc9886d9905745b14dd86d36bde690648e145b))
* **tui:** confirm sends with unreplayable context instead of prompting at model switch ([#1155](https://github.com/matthewyjiang/rho/issues/1155)) ([e575220](https://github.com/matthewyjiang/rho/commit/e575220b69b2ec3e4b6144ced32240bafec21356))
* **tui:** highlight shell commands in tool cards ([#1157](https://github.com/matthewyjiang/rho/issues/1157)) ([69fc3da](https://github.com/matthewyjiang/rho/commit/69fc3da8771d56b512579d00099ad65379056818))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 2.2.0 to 2.3.0

## [2.6.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.5.0...rho-coding-agent-v2.6.0) (2026-09-03)


### Features

* **subagents:** delegate to Cursor Agent with runtime: cursor ([#1144](https://github.com/matthewyjiang/rho/issues/1144)) ([0a1c586](https://github.com/matthewyjiang/rho/commit/0a1c58631f3bf97902dbfae48e8415c3fad4d6f7))
* **subagents:** raise concurrent agent default to 10 and resize live ([#1137](https://github.com/matthewyjiang/rho/issues/1137)) ([a7073bd](https://github.com/matthewyjiang/rho/commit/a7073bd2c9ea598cedc174da19bc83000d47cc40))
* **tui:** add /refresh-models shortcut for provider lists and models.dev catalog ([#1130](https://github.com/matthewyjiang/rho/issues/1130)) ([43779bf](https://github.com/matthewyjiang/rho/commit/43779bfbe3a0c2225c1cbd6495fbab39e98ad7a1))
* **tui:** pick agent tools from a multi-select instead of typing a list ([#1145](https://github.com/matthewyjiang/rho/issues/1145)) ([86499f6](https://github.com/matthewyjiang/rho/commit/86499f638286775361545dd4e5dde2c24e8cab87))


### Bug Fixes

* **compaction:** /compact always shrinks context on large-window models ([#1132](https://github.com/matthewyjiang/rho/issues/1132)) ([d7522b6](https://github.com/matthewyjiang/rho/commit/d7522b68b9c224e11092ad16d95717156b6b7452))
* **tools:** bash tool keeps mise/nix/direnv PATH entries from the launching shell ([#1148](https://github.com/matthewyjiang/rho/issues/1148)) ([589b890](https://github.com/matthewyjiang/rho/commit/589b8905b1c4f6cc45376cdce1bf805c10bef4fc))
* **tui:** stop clipping agent titles on the activity rail ([#1136](https://github.com/matthewyjiang/rho/issues/1136)) ([5381cc3](https://github.com/matthewyjiang/rho/commit/5381cc3deb18d3694dca9a84011958299a7fab5c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 5.2.0 to 5.1.2
    * rho-providers bumped from 2.1.2 to 2.2.0

## [2.5.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.4.0...rho-coding-agent-v2.5.0) (2026-09-02)


### Features

* **tui:** paste clipboard text on right-click ([#1122](https://github.com/matthewyjiang/rho/issues/1122)) ([6008f36](https://github.com/matthewyjiang/rho/commit/6008f36759c0a49aee707e11d161ab7bee0acce0))
* **tui:** show the command on process cards instead of the raw id ([#1124](https://github.com/matthewyjiang/rho/issues/1124)) ([a1c1dec](https://github.com/matthewyjiang/rho/commit/a1c1dec828117d1faa8c13391280d8a06caa57e7))


### Bug Fixes

* **tui:** Kitty image previews no longer reserve blank rows under Herdr ([f11557e](https://github.com/matthewyjiang/rho/commit/f11557e19d5c3033b7b1badb56b23ce219a74375))
* **tui:** require herdr cell metrics before kitty image placements ([69dcfdc](https://github.com/matthewyjiang/rho/commit/69dcfdcc4a13b5c1d2456ed83dd8b5f8207354a0))

## [2.4.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.3.3...rho-coding-agent-v2.4.0) (2026-09-01)


### Features

* **tui:** replace doctor picker with dashboard overlay ([#1119](https://github.com/matthewyjiang/rho/issues/1119)) ([d22c5ee](https://github.com/matthewyjiang/rho/commit/d22c5eed4de4f4f3b6ab49b0f339d9f6bd0e7084))
* **tui:** show live Claude Code usage in /limits ([#1114](https://github.com/matthewyjiang/rho/issues/1114)) ([7f96487](https://github.com/matthewyjiang/rho/commit/7f96487d0325b33a46c080bec4a6073a2a9684b1))


### Bug Fixes

* **limits:** stop spurious Claude usage refresh failures on cold starts ([#1117](https://github.com/matthewyjiang/rho/issues/1117)) ([98b2a24](https://github.com/matthewyjiang/rho/commit/98b2a248dc37cf9507cab64aeae269bb1313a25d))
* **release:** stop release-please failing on the pty harness cycle ([#1116](https://github.com/matthewyjiang/rho/issues/1116)) ([a78805c](https://github.com/matthewyjiang/rho/commit/a78805cc8188085993401294e176025998c34e4c))
* **tui:** hide the caret on dismiss-only overlays ([#1112](https://github.com/matthewyjiang/rho/issues/1112)) ([9089dd0](https://github.com/matthewyjiang/rho/commit/9089dd0edfc4aa0752eb8bec65b1640cec78c6b4))
* **tui:** keep slash commands working after a trailing newline ([#1115](https://github.com/matthewyjiang/rho/issues/1115)) ([a56c19b](https://github.com/matthewyjiang/rho/commit/a56c19be4dfa9c7a352cdc7cc0741552b7a2346d))

## [2.3.3](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.3.2...rho-coding-agent-v2.3.3) (2026-08-31)


### Bug Fixes

* **tui:** keep enter on the bare command after tab completion ([#1109](https://github.com/matthewyjiang/rho/issues/1109)) ([f6b47c4](https://github.com/matthewyjiang/rho/commit/f6b47c4398ee8f7a9cc299a40f27d148e7085f16))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 2.1.1 to 2.1.2

## [2.3.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.3.1...rho-coding-agent-v2.3.2) (2026-08-30)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 5.1.0 to 5.1.1
    * rho-providers bumped from 2.1.0 to 2.1.1

## [2.3.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.3.0...rho-coding-agent-v2.3.1) (2026-08-30)


### Bug Fixes

* **release:** recover provider 2.1.0 ([#1099](https://github.com/matthewyjiang/rho/issues/1099)) ([15f9403](https://github.com/matthewyjiang/rho/commit/15f9403a38c9edf62f7bd46900b05299e99a48ea))
* **tui:** keep images visible across viewport clipping ([#1100](https://github.com/matthewyjiang/rho/issues/1100)) ([4c50ca6](https://github.com/matthewyjiang/rho/commit/4c50ca677a6af8fd177380d90278c7af0ca1fb88))

## [2.3.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.2.0...rho-coding-agent-v2.3.0) (2026-08-30)


### Features

* **auth:** always show a copyable login URL across every provider ([#1095](https://github.com/matthewyjiang/rho/issues/1095)) ([9e2670b](https://github.com/matthewyjiang/rho/commit/9e2670bd0892c579cad5f1d916046c16632ec9e1))
* **tui:** add /side overlay for frozen-context asides ([#1094](https://github.com/matthewyjiang/rho/issues/1094)) ([3ebbdf9](https://github.com/matthewyjiang/rho/commit/3ebbdf951f01c6a32e827d4b819bdd9441c76684))
* **tui:** make MCP tool cards readable instead of raw wire names and JSON blobs ([#1092](https://github.com/matthewyjiang/rho/issues/1092)) ([c57ba6c](https://github.com/matthewyjiang/rho/commit/c57ba6cf79a75d627af37c977c5b2d4f9535a2d7))
* **tui:** oversized mermaid diagrams now clip instead of dumping source ([#1082](https://github.com/matthewyjiang/rho/issues/1082)) ([0f52503](https://github.com/matthewyjiang/rho/commit/0f5250344452dfe4d6aaac537f6874e36ba77cad))
* **tui:** paint gitGraph, gantt, and mindmap in the transcript ([#1088](https://github.com/matthewyjiang/rho/issues/1088)) ([e22adb6](https://github.com/matthewyjiang/rho/commit/e22adb64c84d5480de317290ed8477a180e93b91))
* **tui:** print a compact session receipt after interactive exit ([#1077](https://github.com/matthewyjiang/rho/issues/1077)) ([4f75c2a](https://github.com/matthewyjiang/rho/commit/4f75c2acf885529a960b39df00422a43be8775f5))
* **tui:** stream mermaid diagrams as they arrive instead of popping in at fence close ([#1085](https://github.com/matthewyjiang/rho/issues/1085)) ([29c70cc](https://github.com/matthewyjiang/rho/commit/29c70cc5a0b28f66b13006dac8d3d6e4b01a31fa))


### Bug Fixes

* **mcp:** escape underscore-edged exported tool names ([#1096](https://github.com/matthewyjiang/rho/issues/1096)) ([86c7ea1](https://github.com/matthewyjiang/rho/commit/86c7ea15e28d4ff237823e5f1c26ae22479bcf94))
* **tui:** keep mermaid loops from gluing onto skip edges ([#1087](https://github.com/matthewyjiang/rho/issues/1087)) ([5baa60d](https://github.com/matthewyjiang/rho/commit/5baa60d6e70a422e3af045f2dd25467a5bb07c3f))
* **tui:** make add/remove diff wash visible on sampled themes ([#1086](https://github.com/matthewyjiang/rho/issues/1086)) ([4f660a0](https://github.com/matthewyjiang/rho/commit/4f660a035de61d864b992a643358758d7dee4b56))
* **tui:** place pending input below active work ([#1097](https://github.com/matthewyjiang/rho/issues/1097)) ([c3cf135](https://github.com/matthewyjiang/rho/commit/c3cf135d521fbdf5fc7ba1ecf6ae8973b5648587))
* **tui:** refresh the github pr chip after create ([#1080](https://github.com/matthewyjiang/rho/issues/1080)) ([af367bd](https://github.com/matthewyjiang/rho/commit/af367bd80a5326b26d0670842fb47d48be61cdf4))
* **tui:** render mermaid state start and end without empty boxes ([#1089](https://github.com/matthewyjiang/rho/issues/1089)) ([8545b9f](https://github.com/matthewyjiang/rho/commit/8545b9ff34a940d44a703e81df01dd45461f7be5))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 5.0.0 to 5.1.0
    * rho-providers bumped from 2.1.0 to 3.0.0

## [2.2.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.1.0...rho-coding-agent-v2.2.0) (2026-08-28)


### Features

* **mcp:** allow opt-in cleartext http for remote servers ([#1072](https://github.com/matthewyjiang/rho/issues/1072)) ([15abb91](https://github.com/matthewyjiang/rho/commit/15abb91d34c257be5f9d0ac841bde112f1b6a81a))

## [2.1.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v2.0.0...rho-coding-agent-v2.1.0) (2026-08-27)


### Features

* **tui:** show the current github pr on the statusline ([#1067](https://github.com/matthewyjiang/rho/issues/1067)) ([cccee94](https://github.com/matthewyjiang/rho/commit/cccee943c17250727447f691aac6f736e4890eb7))

## [2.0.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.49.0...rho-coding-agent-v2.0.0) (2026-08-27)


### ⚠ BREAKING CHANGES

* rho-sdk removes RunEvent::ProviderActivity and the PROVIDER_ACTIVITY_* constants, collapses ProviderStreamResetReason retryable variants into RetryableFailure { kind, retry_after }, removes ModelCallMetrics::output_tokens_per_second, adds revision to RunEvent::Failed, folds ContextEstimated into StepStarted, and replaces the stringly generation-output-token carriers with a typed ModelCallMetrics::generation_output_tokens metric and ModelEvent::GenerationOutputTokens variant. rho-providers adds ProviderId::OpenAiCompatible for config-defined hosts, folds CustomProviderOptions into CustomProviderSpec (removing the _with_lookup/ _with_options funnels), makes providers() include custom hosts (removing visible_providers), removes deprecated registry API-base re-exports and ProviderRuntime::same_family, and drops the CredentialStore parameter from OpenAiProvider::new_with_identity. rho-tools removes the edit_file tool-name alias, the write_file frontmatter alias, and the EditToolKind type alias.
* **providers:** own HTTP transport errors and collapse reqwest onto 0.13 ([#1060](https://github.com/matthewyjiang/rho/issues/1060))

### Features

* **providers:** add MiniMax as a first-party Anthropic-compatible host ([#1059](https://github.com/matthewyjiang/rho/issues/1059)) ([aa71132](https://github.com/matthewyjiang/rho/commit/aa7113215ed9601dd9bf74241a031febd9ee2c85))
* **tui:** create agents from a guided slash command ([#1057](https://github.com/matthewyjiang/rho/issues/1057)) ([b14e4d9](https://github.com/matthewyjiang/rho/commit/b14e4d9ca92c50b3c4ab7079d7cc1ed5aac3359f))
* **tui:** queue follow-ups with Ctrl+Enter when Alt+Enter is fullscreen ([#1064](https://github.com/matthewyjiang/rho/issues/1064)) ([67cd423](https://github.com/matthewyjiang/rho/commit/67cd423e8d7f8f99e23eafea425d99df53aaa910))


### Bug Fixes

* **ci:** repair macOS stdin guard and supervised approval PTY smoke ([#1055](https://github.com/matthewyjiang/rho/issues/1055)) ([8b6654f](https://github.com/matthewyjiang/rho/commit/8b6654ff86b34cd4f2a41d11520085f0a1ea5c01))
* **tui:** keep activity rail text readable on terminal themes ([#1065](https://github.com/matthewyjiang/rho/issues/1065)) ([98bb0eb](https://github.com/matthewyjiang/rho/commit/98bb0ebc1a6f1e72bac66925a0e424dd415de10f))
* **tui:** keep the history scrollbar from darkening on tool cards ([#1063](https://github.com/matthewyjiang/rho/issues/1063)) ([5f69a8c](https://github.com/matthewyjiang/rho/commit/5f69a8cee685f8500e5c2bc17b41021d98ca7afa))


### Performance Improvements

* **tls:** drop OpenSSL and run rustls+ring everywhere ([#1061](https://github.com/matthewyjiang/rho/issues/1061)) ([3ab2016](https://github.com/matthewyjiang/rho/commit/3ab20166d06d805a9554786904dc16ec2544a2c7))


### Code Refactoring

* pay down all NEXT_MAJOR debt for the next major, net -986 lines ([#1062](https://github.com/matthewyjiang/rho/issues/1062)) ([0e17bdb](https://github.com/matthewyjiang/rho/commit/0e17bdb5e9fa21440fffbbe00171830ac0d1d3c2))
* **providers:** own HTTP transport errors and collapse reqwest onto 0.13 ([#1060](https://github.com/matthewyjiang/rho/issues/1060)) ([701dd29](https://github.com/matthewyjiang/rho/commit/701dd297d1b3e17df55f9dbf6f13e9c2a1d0ccb5))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.11.0 to 2.0.0

## [1.49.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.48.0...rho-coding-agent-v1.49.0) (2026-08-26)


### Features

* **tui:** make the activity rail a trustworthy ambient view of agents and jobs ([#1049](https://github.com/matthewyjiang/rho/issues/1049)) ([d1c2289](https://github.com/matthewyjiang/rho/commit/d1c2289d8fc1d94212d5dbcd46bac97a71513010))


### Bug Fixes

* **tui:** clear bare slash when Esc dismisses command palette ([#1054](https://github.com/matthewyjiang/rho/issues/1054)) ([465897d](https://github.com/matthewyjiang/rho/commit/465897de2b9175215cf6f71531f601ffe0c95926))
* **tui:** keep critical context fill visible in narrow statuslines ([#1052](https://github.com/matthewyjiang/rho/issues/1052)) ([e497dbd](https://github.com/matthewyjiang/rho/commit/e497dbd1dc9103fd1e35cfde4cf8d7f34b37c120))
* **tui:** unify key hints, picker titles, and transcript errors ([#1053](https://github.com/matthewyjiang/rho/issues/1053)) ([8dbb2a5](https://github.com/matthewyjiang/rho/commit/8dbb2a568263234c6eb4140df34252a04f27e93c))
* **tui:** use plain language in session delete confirms ([#1051](https://github.com/matthewyjiang/rho/issues/1051)) ([55ccd83](https://github.com/matthewyjiang/rho/commit/55ccd83d3c2b700180c57668a548dcb0e9cc297e))

## [1.48.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.47.1...rho-coding-agent-v1.48.0) (2026-08-25)


### Features

* **attach:** show reasoning level next to model and provider ([#1044](https://github.com/matthewyjiang/rho/issues/1044)) ([2b1f912](https://github.com/matthewyjiang/rho/commit/2b1f912e1233b5769e250a4bf53538661afa9126))
* **providers:** let custom hosts speak OpenAI Responses ([#1041](https://github.com/matthewyjiang/rho/issues/1041)) ([381383d](https://github.com/matthewyjiang/rho/commit/381383d9f5f5d13746e23518d152075b63e6128d))
* **tui:** explain hidden tool cards in zen mode ([#1037](https://github.com/matthewyjiang/rho/issues/1037)) ([f523866](https://github.com/matthewyjiang/rho/commit/f523866e8cf9dd459cedde2cfb8858700944f1d0))
* **tui:** pick Chat Completions or Responses when adding a custom host ([#1042](https://github.com/matthewyjiang/rho/issues/1042)) ([586c995](https://github.com/matthewyjiang/rho/commit/586c9954134f1b241ee6e5d8a31056d22977be8e))
* **tui:** show how long a turn took ([#1038](https://github.com/matthewyjiang/rho/issues/1038)) ([7b45897](https://github.com/matthewyjiang/rho/commit/7b458979af6250f159e26005acef0988ccb5bd5a))


### Bug Fixes

* **permission:** stop silent reads of files outside the workspace ([#1033](https://github.com/matthewyjiang/rho/issues/1033)) ([21e5e20](https://github.com/matthewyjiang/rho/commit/21e5e20d5727b6c72132c819495114d3d9788150))
* **plugins:** gate project plugins behind workspace trust ([#1030](https://github.com/matthewyjiang/rho/issues/1030)) ([60297fc](https://github.com/matthewyjiang/rho/commit/60297fc5396f53729f78c0b5e034d96748fc9abe))
* **providers:** keep prompt-cache hits across tool turns ([#1040](https://github.com/matthewyjiang/rho/issues/1040)) ([e139d99](https://github.com/matthewyjiang/rho/commit/e139d99e6abb4e0ee9315305958e357da3afe715))
* **tui:** applied steers now show up in the transcript ([#1032](https://github.com/matthewyjiang/rho/issues/1032)) ([48ee37f](https://github.com/matthewyjiang/rho/commit/48ee37f4aa21aae896a02c24e4eadaaad1242523))
* **tui:** keep empty /model guidance in the transcript ([#1034](https://github.com/matthewyjiang/rho/issues/1034)) ([42e7e87](https://github.com/matthewyjiang/rho/commit/42e7e8792786a2f16373181b7a9acd2c8a9e6dfd))
* **tui:** keep Thinking... on the Thought for row ([#1047](https://github.com/matthewyjiang/rho/issues/1047)) ([9f01d0b](https://github.com/matthewyjiang/rho/commit/9f01d0b560644d8707cc6164e229c9d03defd4c5))
* **tui:** keep Worked for off the last line of the reply ([#1046](https://github.com/matthewyjiang/rho/issues/1046)) ([eb3099d](https://github.com/matthewyjiang/rho/commit/eb3099d941305268ea3d28194392af6c38a62dd6))
* **tui:** make active-turn controls visible ([#1028](https://github.com/matthewyjiang/rho/issues/1028)) ([1246e44](https://github.com/matthewyjiang/rho/commit/1246e4458ab842384c21de9de935f2da5ff5697a))


### Performance Improvements

* cut hot-path costs in grep, session persistence, and live TUI rendering ([#1027](https://github.com/matthewyjiang/rho/issues/1027)) ([9cf4ad7](https://github.com/matthewyjiang/rho/commit/9cf4ad73149ebcaffbe760481618cac9bb02a061))
* **tui:** keep long streamed replies from stalling on open fences ([#1043](https://github.com/matthewyjiang/rho/issues/1043)) ([4d5adfa](https://github.com/matthewyjiang/rho/commit/4d5adfab9421f98f987eee7a9f1e0d03de6e53d1))

## [1.47.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.47.0...rho-coding-agent-v1.47.1) (2026-08-22)


### Bug Fixes

* **process:** terminate process tree when shell leader exits ([#1023](https://github.com/matthewyjiang/rho/issues/1023)) ([d53747e](https://github.com/matthewyjiang/rho/commit/d53747e16ba2553543de4c21493d16c8d8a9f228))

## [1.47.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.46.0...rho-coding-agent-v1.47.0) (2026-08-21)


### Features

* **acp:** let hosts discover and switch models per-session ([#1012](https://github.com/matthewyjiang/rho/issues/1012)) ([9ddaf37](https://github.com/matthewyjiang/rho/commit/9ddaf373c90565248896044e57fa9d7da8c8efee))
* **acp:** let hosts set reasoning through thought_level ([#1015](https://github.com/matthewyjiang/rho/issues/1015)) ([5cda8e8](https://github.com/matthewyjiang/rho/commit/5cda8e8308682fa13c9d6001ec12989f0d030bb4))
* **attach:** show live tok/s in the subagent attach header ([#1011](https://github.com/matthewyjiang/rho/issues/1011)) ([75f2db5](https://github.com/matthewyjiang/rho/commit/75f2db5dd9c61e07436f2d4775111fb9e9b6b98c))
* **tui:** bundle popular color schemes as built-in themes ([#1014](https://github.com/matthewyjiang/rho/issues/1014)) ([b0a1ad6](https://github.com/matthewyjiang/rho/commit/b0a1ad622f19684d1b47e528171e38225031c04e))
* **tui:** flag finished turns on the jump chip and toast collapsed pastes ([#1021](https://github.com/matthewyjiang/rho/issues/1021)) ([a16a4d4](https://github.com/matthewyjiang/rho/commit/a16a4d4de94ac1c09a39ed9970dce0cf74053569))


### Bug Fixes

* **subagents:** tolerate concurrent session subagents dir creation ([#1020](https://github.com/matthewyjiang/rho/issues/1020)) ([0a68625](https://github.com/matthewyjiang/rho/commit/0a68625102e153321119d98ca7655e18e36e6890))
* **tui:** show context tokens when no window limit is reported ([#1019](https://github.com/matthewyjiang/rho/issues/1019)) ([a4d8909](https://github.com/matthewyjiang/rho/commit/a4d890987535c09c1ac8469977e87c1100b6a9df))


### Performance Improvements

* cut per-frame palette discovery, layout rework, and git spawn churn ([#1017](https://github.com/matthewyjiang/rho/issues/1017)) ([39952ce](https://github.com/matthewyjiang/rho/commit/39952ce4177da242f172b6a6bbcc30721424924b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 4.1.1 to 4.2.0
    * rho-providers bumped from 1.9.0 to 1.9.1

## [1.46.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.45.0...rho-coding-agent-v1.46.0) (2026-08-20)


### Features

* **cli:** open the tui with the first prompt already running ([#1003](https://github.com/matthewyjiang/rho/issues/1003)) ([20b9d43](https://github.com/matthewyjiang/rho/commit/20b9d43b4dcb71f231f1022e7c2e4c4d48a8a9b4))
* **providers:** look up models.dev from custom host model ids ([#1004](https://github.com/matthewyjiang/rho/issues/1004)) ([da477da](https://github.com/matthewyjiang/rho/commit/da477da84b401f3bc2b299e481851dfd5278aded))
* **tools:** mint hashline tags only for the hashline edit tool ([#1000](https://github.com/matthewyjiang/rho/issues/1000)) ([2fa3c2c](https://github.com/matthewyjiang/rho/commit/2fa3c2c35280f79cb53826cf3cc863421fcd18d0))
* **tui:** add /copy for the last assistant message ([#998](https://github.com/matthewyjiang/rho/issues/998)) ([991c147](https://github.com/matthewyjiang/rho/commit/991c1471a94e67260ef3bad3aa986166903920df))
* **tui:** watch subagents in the same terminal ([#1002](https://github.com/matthewyjiang/rho/issues/1002)) ([966f3e8](https://github.com/matthewyjiang/rho/commit/966f3e8315f8d2c39f91ef31577aa4b81f3a1e80))


### Bug Fixes

* **tui:** filter /agents reasoning choices by models.dev capabilities ([#1005](https://github.com/matthewyjiang/rho/issues/1005)) ([098a9d1](https://github.com/matthewyjiang/rho/commit/098a9d1d7d4c0aacd64ee734262fa51a78b0e1ed))
* **tui:** stop openai-compatible cost jumping on submit ([#1008](https://github.com/matthewyjiang/rho/issues/1008)) ([51f881f](https://github.com/matthewyjiang/rho/commit/51f881f68d663b1d15a0b280f0fef76cbcf9a8ba))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 4.1.0 to 4.1.1
    * rho-providers bumped from 1.8.0 to 1.9.0

## [1.45.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.44.0...rho-coding-agent-v1.45.0) (2026-08-18)


### Features

* **providers:** read Ollama context and thinking from native tags ([#993](https://github.com/matthewyjiang/rho/issues/993)) ([dfd0f01](https://github.com/matthewyjiang/rho/commit/dfd0f0145ac26a34365f16446bc924ef1de125b5))
* **providers:** set up ollama through /login instead of first-run defaults ([#994](https://github.com/matthewyjiang/rho/issues/994)) ([1fbe5f8](https://github.com/matthewyjiang/rho/commit/1fbe5f853d9f9b8f02251b5e9dbad2258e82a20c))
* **tui:** cycle pinned models without opening the catalogue ([#988](https://github.com/matthewyjiang/rho/issues/988)) ([c0d0292](https://github.com/matthewyjiang/rho/commit/c0d029261852a91ae24ec5ced355de8ef7c876f9))
* **tui:** show prompt-cache misses in /info and optional notices ([#989](https://github.com/matthewyjiang/rho/issues/989)) ([4b35cb6](https://github.com/matthewyjiang/rho/commit/4b35cb661a75e355bd0405a39a7b1dcace7a1c43))


### Bug Fixes

* **providers:** keep a stored custom key after /login and restart ([#995](https://github.com/matthewyjiang/rho/issues/995)) ([5e404f6](https://github.com/matthewyjiang/rho/commit/5e404f6d584dcfe1ec22ffbe2fa2b472754d6cca))
* **tui:** keep picker keybinds from clipping mid-hint ([#996](https://github.com/matthewyjiang/rho/issues/996)) ([f7bca1e](https://github.com/matthewyjiang/rho/commit/f7bca1eedbb2c6f1b86bc14426a5d30948b9d614))
* **tui:** keep up-arrow history out of the command palette ([#990](https://github.com/matthewyjiang/rho/issues/990)) ([3a5b048](https://github.com/matthewyjiang/rho/commit/3a5b0481d6f00e54862b48af6cfb37cd31549631))

## [1.44.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.43.1...rho-coding-agent-v1.44.0) (2026-08-18)


### Features

* **providers:** let /login add custom hosts that work with CLIProxyAPI ([#984](https://github.com/matthewyjiang/rho/issues/984)) ([5ec9a44](https://github.com/matthewyjiang/rho/commit/5ec9a445db5ff9c47cc814d2cbd282172c827a5a))
* **tui:** keep up-arrow prompts across sessions ([#982](https://github.com/matthewyjiang/rho/issues/982)) ([5ec18c2](https://github.com/matthewyjiang/rho/commit/5ec18c240d49a4473203b10969b7e3445873a4e4))
* **tui:** regroup the /config category browser ([#983](https://github.com/matthewyjiang/rho/issues/983)) ([bf0008b](https://github.com/matthewyjiang/rho/commit/bf0008bbef289d607f7fdaf2572de7005fd8efc8))


### Performance Improvements

* **tools:** cut tokens from web, process, and shell results ([#980](https://github.com/matthewyjiang/rho/issues/980)) ([c103ec0](https://github.com/matthewyjiang/rho/commit/c103ec0d6d46896f735074413369cc471c95a750))
* **tui:** make rho attach scrolling smooth on long transcripts ([#985](https://github.com/matthewyjiang/rho/issues/985)) ([8e1e243](https://github.com/matthewyjiang/rho/commit/8e1e2430c026d9adc6b968dfed0ab89fcdc5513e))

## [1.43.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.43.0...rho-coding-agent-v1.43.1) (2026-08-17)


### Performance Improvements

* **tui:** keep first resume paint off the syntax dump ([#972](https://github.com/matthewyjiang/rho/issues/972)) ([81aa8fa](https://github.com/matthewyjiang/rho/commit/81aa8fa0248255efa6567a83095b05123a773b10))
* **web:** stop keeping every fetched page in ram ([#977](https://github.com/matthewyjiang/rho/issues/977)) ([4b392e8](https://github.com/matthewyjiang/rho/commit/4b392e8c1dbeda2bbd7cbe206320962100514a60))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.6.0 to 1.6.1

## [1.43.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.42.0...rho-coding-agent-v1.43.0) (2026-08-17)


### Features

* **tui:** show the advisor on the composer divider ([#968](https://github.com/matthewyjiang/rho/issues/968)) ([3af67ce](https://github.com/matthewyjiang/rho/commit/3af67ce3da780dedc0fe35c28e82bcaf0fc7e858))
* **xai:** let grok generate and edit images in the conversation ([#967](https://github.com/matthewyjiang/rho/issues/967)) ([47c03c1](https://github.com/matthewyjiang/rho/commit/47c03c1d2b13864d32a69b27c21da4db54572e0e))


### Bug Fixes

* **advisor:** discourage first-turn advisor calls ([#969](https://github.com/matthewyjiang/rho/issues/969)) ([cec3198](https://github.com/matthewyjiang/rho/commit/cec3198841c18ae9d75459f3efd3c3a6f38ab011))
* **advisor:** stop the reviewer from inventing empty-call failures ([#961](https://github.com/matthewyjiang/rho/issues/961)) ([061fbbe](https://github.com/matthewyjiang/rho/commit/061fbbe7040eebbc1caa07021b1e1c609fc6ea09))
* **models:** stop clamping GPT-5.5 and GPT-5.6 context to 272k ([#966](https://github.com/matthewyjiang/rho/issues/966)) ([c5984e5](https://github.com/matthewyjiang/rho/commit/c5984e5d04d6f4ab1d24c250cb6b2e74c4362187))


### Performance Improvements

* **session:** keep one transcript in ram instead of one per turn ([#964](https://github.com/matthewyjiang/rho/issues/964)) ([c2a0c7d](https://github.com/matthewyjiang/rho/commit/c2a0c7d1a82036743789df3d571f9e062840bfa4))
* **tui:** stop keeping a second transcript after resume ([#965](https://github.com/matthewyjiang/rho/issues/965)) ([52fd5c6](https://github.com/matthewyjiang/rho/commit/52fd5c68457c9114d258148c2ea8c10e86b70e4e))

## [1.42.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.41.0...rho-coding-agent-v1.42.0) (2026-08-16)


### Features

* **tui:** add /clear and /usage command aliases ([#942](https://github.com/matthewyjiang/rho/issues/942)) ([312ada1](https://github.com/matthewyjiang/rho/commit/312ada1608d93984904e1b3481d86b8aa0128397))
* **tui:** pick running subagents by title instead of run id ([#950](https://github.com/matthewyjiang/rho/issues/950)) ([dffd205](https://github.com/matthewyjiang/rho/commit/dffd2053eb4d22bc0b86d364c31fc44c8317667d))


### Bug Fixes

* **attach:** list only this directory's subagents in the picker ([#951](https://github.com/matthewyjiang/rho/issues/951)) ([9fc35d9](https://github.com/matthewyjiang/rho/commit/9fc35d99b6c0d004788d802cacfd3fd8920210d9))
* **attach:** open an empty picker and include finished runs ([#955](https://github.com/matthewyjiang/rho/issues/955)) ([7055695](https://github.com/matthewyjiang/rho/commit/7055695654674f4b051e3adcc589ed6adb049acf))
* **tui:** keep full resume scrollback ([#956](https://github.com/matthewyjiang/rho/issues/956)) ([553f3fc](https://github.com/matthewyjiang/rho/commit/553f3fc0d5305189f6cadf5c003fd2d5b6eb4b89))
* **tui:** let you cancel claude-code login before the handoff ([#953](https://github.com/matthewyjiang/rho/issues/953)) ([849ba3b](https://github.com/matthewyjiang/rho/commit/849ba3b74ad3eb34afd20e0bb9ab1e17addb4d72))
* **tui:** let you type while compact runs on the main loop ([#957](https://github.com/matthewyjiang/rho/issues/957)) ([07ef7c1](https://github.com/matthewyjiang/rho/commit/07ef7c14719427e4a77a23056761ca6f7532954f))


### Performance Improvements

* shrink release binary 38% and stop repeating startup work ([#947](https://github.com/matthewyjiang/rho/issues/947)) ([3fc397d](https://github.com/matthewyjiang/rho/commit/3fc397dd8d2926c20354471741a4d74a873a82c1))
* **tui:** paint the first frame before MCP connect and other startup tails ([#948](https://github.com/matthewyjiang/rho/issues/948)) ([cdc1bbf](https://github.com/matthewyjiang/rho/commit/cdc1bbfeabd93bf2bf5b615911048a2eebb6a947))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.4.0 to 1.5.0

## [1.41.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.40.1...rho-coding-agent-v1.41.0) (2026-08-15)


### Features

* **tui:** lift tool-card text on hover in main TUI and attach ([#928](https://github.com/matthewyjiang/rho/issues/928)) ([344f79a](https://github.com/matthewyjiang/rho/commit/344f79a49cf5c4fa754c87a18d6b53b34bcffe5d))
* **tui:** show /limits in a single-pane overlay ([#940](https://github.com/matthewyjiang/rho/issues/940)) ([273b830](https://github.com/matthewyjiang/rho/commit/273b830d42b8225eb7dbc416515aacbeb1567590))
* **tui:** show live background processes in the activity rail ([#935](https://github.com/matthewyjiang/rho/issues/935)) ([f396b72](https://github.com/matthewyjiang/rho/commit/f396b72542abf17c2ef108c91bfa04351125031a))
* **tui:** show OpenCode Go usage bars in /limits ([#930](https://github.com/matthewyjiang/rho/issues/930)) ([2ea9628](https://github.com/matthewyjiang/rho/commit/2ea9628e05f6118677bc7f508d125a389150c98e))
* **tui:** wake the agent when a background process exits ([#934](https://github.com/matthewyjiang/rho/issues/934)) ([b3b4ada](https://github.com/matthewyjiang/rho/commit/b3b4adac05f09390f475b7a9bde2d07976d09d5d))


### Bug Fixes

* **cli:** show changelog for dependency-only releases ([#919](https://github.com/matthewyjiang/rho/issues/919)) ([3488499](https://github.com/matthewyjiang/rho/commit/34884997edf2f582cbd1952ef530214786f19b51))
* **session:** v1 upgrades no longer write an unloadable transcript ([#939](https://github.com/matthewyjiang/rho/issues/939)) ([1dc1002](https://github.com/matthewyjiang/rho/commit/1dc1002c5c60ddc2a2889a12d92ead905cd4b6d3))


### Performance Improvements

* **session:** eliminate double transcript deserialization and message clones during turn save ([#936](https://github.com/matthewyjiang/rho/issues/936)) ([4c60e8d](https://github.com/matthewyjiang/rho/commit/4c60e8d346d8318bee6bbde984e69bcee2049bf3))
* **tui:** stop recompiling search regex on every painted line ([#938](https://github.com/matthewyjiang/rho/issues/938)) ([74689ae](https://github.com/matthewyjiang/rho/commit/74689ae30ec784b5a49fcbfb3df8e0bcddc0fd4e))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 4.0.0 to 4.0.1
    * rho-providers bumped from 1.3.1 to 1.4.0

## [1.40.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.40.0...rho-coding-agent-v1.40.1) (2026-08-14)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.3.0 to 1.3.1

## [1.40.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.39.1...rho-coding-agent-v1.40.0) (2026-08-14)


### Features

* **attach:** expand tool cards on click and ctrl+o ([#911](https://github.com/matthewyjiang/rho/issues/911)) ([061bf10](https://github.com/matthewyjiang/rho/commit/061bf10e1aa28b435eac2a9a46458074ec2c15f3))
* **claude-cli:** show Claude tool names on attach cards ([#909](https://github.com/matthewyjiang/rho/issues/909)) ([1bcdaba](https://github.com/matthewyjiang/rho/commit/1bcdabaf2bdf1a5d991adf6330b95f1bddaf503f))
* **cli:** let editor hosts drive Rho over stdio ([#910](https://github.com/matthewyjiang/rho/issues/910)) ([3f10136](https://github.com/matthewyjiang/rho/commit/3f10136d39ed6a428a4b4e7dca40c2fec882fe67))
* **providers:** sign in to OpenCode Go and use its models ([#913](https://github.com/matthewyjiang/rho/issues/913)) ([db501a6](https://github.com/matthewyjiang/rho/commit/db501a6ab5b64c44114d6ecf1a1bf3a75ac597b6))


### Bug Fixes

* **anthropic:** derive reasoning levels from models api capabilities ([#907](https://github.com/matthewyjiang/rho/issues/907)) ([f057529](https://github.com/matthewyjiang/rho/commit/f057529128be880c640e5dd9b5d5e42071eed792))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.2.2 to 1.3.0

## [1.39.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.39.0...rho-coding-agent-v1.39.1) (2026-08-14)


### Bug Fixes

* **anthropic:** stop opus 5 from sending thinking.type.enabled ([#904](https://github.com/matthewyjiang/rho/issues/904)) ([d76a398](https://github.com/matthewyjiang/rho/commit/d76a3984d37d299850de6fabf9d7135c047c137c))
* **claude-cli:** keep Auto dontAsk on the bound tool set ([#903](https://github.com/matthewyjiang/rho/issues/903)) ([fb2de07](https://github.com/matthewyjiang/rho/commit/fb2de0793939ce4034303718efadb03fba858ab9))
* **skills:** load skills with nested metadata ([#901](https://github.com/matthewyjiang/rho/issues/901)) ([b75c6e6](https://github.com/matthewyjiang/rho/commit/b75c6e69ad1310a18dd54f3900f8f8f323ff9c47))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.2.0 to 1.2.1

## [1.39.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.38.2...rho-coding-agent-v1.39.0) (2026-08-13)


### Features

* **permission:** let Auto skip review of tracked workspace edits ([#892](https://github.com/matthewyjiang/rho/issues/892)) ([2babff2](https://github.com/matthewyjiang/rho/commit/2babff25ddd4788433adbdb40babf38a59c4ec29))
* **permission:** screen Auto requests with a two-stage classifier ([#893](https://github.com/matthewyjiang/rho/issues/893)) ([4149f11](https://github.com/matthewyjiang/rho/commit/4149f1157aa2c8ee10561a21a919a0e530b8f3cc))
* **providers:** let config name openai-compatible hosts ([#888](https://github.com/matthewyjiang/rho/issues/888)) ([a87649a](https://github.com/matthewyjiang/rho/commit/a87649a9e76332f53f125c52e7eccd8a14bc14f1))

## [1.38.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.38.1...rho-coding-agent-v1.38.2) (2026-08-12)


### Bug Fixes

* **xai:** add grok-4.6 to the static model allowlist ([#882](https://github.com/matthewyjiang/rho/issues/882)) ([52ebd71](https://github.com/matthewyjiang/rho/commit/52ebd716456edcd9bd41d44d35afc7fc283cb31c))


### Performance Improvements

* **tui:** scale render hot paths for long transcripts ([#876](https://github.com/matthewyjiang/rho/issues/876)) ([62d2500](https://github.com/matthewyjiang/rho/commit/62d25006e104887654962c821578c4e1158b1425))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 1.1.0 to 1.1.1

## [1.38.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.38.0...rho-coding-agent-v1.38.1) (2026-08-12)


### Bug Fixes

* **tui:** prompt for Auto classifier model at startup ([#877](https://github.com/matthewyjiang/rho/issues/877)) ([c8cba48](https://github.com/matthewyjiang/rho/commit/c8cba4879f6e7b698412ee098ee86ea506deed85))

## [1.38.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.37.0...rho-coding-agent-v1.38.0) (2026-08-12)


### Features

* **attach:** honor hide-reasoning and other display config ([#873](https://github.com/matthewyjiang/rho/issues/873)) ([813f22f](https://github.com/matthewyjiang/rho/commit/813f22ff4651da1c716ad13dcb6e4553ca3f117d))
* **permission:** rename Auto to Bypass and add classifier Auto ([#870](https://github.com/matthewyjiang/rho/issues/870)) ([3192daa](https://github.com/matthewyjiang/rho/commit/3192daa713f7202f44727ec4acb83d0c646d1286))
* **tui:** count up shell runtime next to timeout ([#871](https://github.com/matthewyjiang/rho/issues/871)) ([7c55124](https://github.com/matthewyjiang/rho/commit/7c55124aa43fb40db414c7be01ab68e36dfb0441))


### Performance Improvements

* **tui:** stop long transcripts rebuilding on image-budget jitter ([#874](https://github.com/matthewyjiang/rho/issues/874)) ([c888e94](https://github.com/matthewyjiang/rho/commit/c888e946599dba4f9afcf01d1d4f4927772f8249))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 3.1.0 to 4.0.0
    * rho-providers bumped from 1.0.0 to 1.1.0

## [1.37.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.36.0...rho-coding-agent-v1.37.0) (2026-08-12)


### Features

* **tui:** keep Entry::Error readable without color ([#866](https://github.com/matthewyjiang/rho/issues/866)) ([1cf70d2](https://github.com/matthewyjiang/rho/commit/1cf70d2f13b85651131864f22cc2b06f1755ea02))
* **tui:** soft-wash diff add/remove rows with readable content ([#867](https://github.com/matthewyjiang/rho/issues/867)) ([bfccb02](https://github.com/matthewyjiang/rho/commit/bfccb02516e71d187f4799d3692c9750a19e9c09))

## [1.36.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.35.0...rho-coding-agent-v1.36.0) (2026-08-11)


### Features

* **prompt:** tell the agent which model runs it, its subagents, and the advisor ([#860](https://github.com/matthewyjiang/rho/issues/860)) ([d18c377](https://github.com/matthewyjiang/rho/commit/d18c3774a20657cc1214e2936251cc993b69dd14))


### Bug Fixes

* **prompt:** wait for catalog names before model labels ([#863](https://github.com/matthewyjiang/rho/issues/863)) ([71fa544](https://github.com/matthewyjiang/rho/commit/71fa544e86e5dd046b898be76632996d92915c19))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.21.0 to 1.0.0

## [1.35.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.34.1...rho-coding-agent-v1.35.0) (2026-08-11)


### Features

* **prompt:** prefer top-down mermaid flowcharts ([#849](https://github.com/matthewyjiang/rho/issues/849)) ([ed7d59a](https://github.com/matthewyjiang/rho/commit/ed7d59ae39d24ddc7a08a69f441309c126ca0981))
* **prompt:** put session cwd in the system prompt ([#857](https://github.com/matthewyjiang/rho/issues/857)) ([e614d9b](https://github.com/matthewyjiang/rho/commit/e614d9b942869caa6cd36139bc08ad038c63b982))
* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))
* **subagents:** enable parent messaging for claude-cli children ([#854](https://github.com/matthewyjiang/rho/issues/854)) ([22caf60](https://github.com/matthewyjiang/rho/commit/22caf6070f32b5152913255bbdd050091eba90cc))
* **tui:** scale feed image height with the history viewport ([#856](https://github.com/matthewyjiang/rho/issues/856)) ([3bee525](https://github.com/matthewyjiang/rho/commit/3bee5251f7f2e61e19cbdf7cfd8c85d32b891ac8))


### Bug Fixes

* **advisor:** stop Claude plan mode from poisoning guidance ([#853](https://github.com/matthewyjiang/rho/issues/853)) ([5209474](https://github.com/matthewyjiang/rho/commit/52094742d99a2f53fa2bafb377f4df44abba0f39))
* **subagents:** allow questionnaire on parent-bridged delegated runs ([#851](https://github.com/matthewyjiang/rho/issues/851)) ([127510b](https://github.com/matthewyjiang/rho/commit/127510bd0a7098233f5d46ee7b7a13f82e754f87))
* **tui:** stop link underlines from spilling into gutters ([#858](https://github.com/matthewyjiang/rho/issues/858)) ([fdde28c](https://github.com/matthewyjiang/rho/commit/fdde28cd28eb5913e4a1b5dea2ca742fa29e552c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 2.1.0 to 3.0.0
    * rho-providers bumped from 0.19.1 to 0.20.0

## [1.34.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.34.0...rho-coding-agent-v1.34.1) (2026-08-10)


### Bug Fixes

* **claude:** surface stream-json API errors on nonzero exit ([#847](https://github.com/matthewyjiang/rho/issues/847)) ([29ac39c](https://github.com/matthewyjiang/rho/commit/29ac39cd3b59e4bd9c7b850488cc108b9a0b41ba))
* **tui:** lead approval prompts with the command ([#846](https://github.com/matthewyjiang/rho/issues/846)) ([413063c](https://github.com/matthewyjiang/rho/commit/413063c9d169e10ca369d83172fbbb952619f07c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.19.0 to 0.19.1

## [1.34.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.33.1...rho-coding-agent-v1.34.0) (2026-08-10)


### Features

* **advisor:** run the advisor on Claude Code from the model picker ([#833](https://github.com/matthewyjiang/rho/issues/833)) ([b663739](https://github.com/matthewyjiang/rho/commit/b6637391a4ade6edcc14bc5b98d7f7790d5a0a68))
* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))
* **mcp:** add prompts, resources, elicitation, and sampling ([#837](https://github.com/matthewyjiang/rho/issues/837)) ([29cf0ea](https://github.com/matthewyjiang/rho/commit/29cf0ea8e6179df94826c18b857cf6f20120be3e))
* **mcp:** answer server-initiated protocol traffic ([#834](https://github.com/matthewyjiang/rho/issues/834)) ([95104ea](https://github.com/matthewyjiang/rho/commit/95104ea6f4bb910604688b69b0724cccc90dde9a))
* **mcp:** authorize remote servers with OAuth 2.1 ([#838](https://github.com/matthewyjiang/rho/issues/838)) ([1971a19](https://github.com/matthewyjiang/rho/commit/1971a19a0e425f7a2b4887757b2a25c6e250ffca))
* **mcp:** render tool results by content kind ([#836](https://github.com/matthewyjiang/rho/issues/836)) ([9ac86ea](https://github.com/matthewyjiang/rho/commit/9ac86ead85ce742a1d3567f94000174ca636a0ab))
* **tui:** make the composer mouse-selectable ([#832](https://github.com/matthewyjiang/rho/issues/832)) ([3cc9490](https://github.com/matthewyjiang/rho/commit/3cc9490f724a077ea69f19fd8599eaefeb7aac11))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.18.0 to 2.0.0
    * rho-providers bumped from 0.18.2 to 0.19.0

## [1.33.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.33.0...rho-coding-agent-v1.33.1) (2026-08-08)


### Bug Fixes

* **tui:** compile smoke_injection for release chrome version ([#828](https://github.com/matthewyjiang/rho/issues/828)) ([3ef3357](https://github.com/matthewyjiang/rho/commit/3ef335773845e583789c1540ad5498e35a38c883))

## [1.33.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.32.2...rho-coding-agent-v1.33.0) (2026-08-08)


### Features

* **tools:** add selectable edit formats ([#820](https://github.com/matthewyjiang/rho/issues/820)) ([5db37d0](https://github.com/matthewyjiang/rho/commit/5db37d0bf714afbf28e91e3f0e66c2e8b807b659))
* **tui:** add /sessions hub for cross-directory session management ([#818](https://github.com/matthewyjiang/rho/issues/818)) ([5a933db](https://github.com/matthewyjiang/rho/commit/5a933db1cb5c1c6029234a8be8ef54eaa1f824b6))
* **tui:** borderless code, mermaid, and math blocks with syntect highlighting ([#825](https://github.com/matthewyjiang/rho/issues/825)) ([f0614a8](https://github.com/matthewyjiang/rho/commit/f0614a88d035f0465adbddccc830207d5366450f))
* **tui:** selectable color themes with live preview ([#817](https://github.com/matthewyjiang/rho/issues/817)) ([c43ba0e](https://github.com/matthewyjiang/rho/commit/c43ba0efbd89ee7eb88ff4716f56697c346737ff))
* **tui:** show runtime and elapsed in attach header ([#816](https://github.com/matthewyjiang/rho/issues/816)) ([0ab34fc](https://github.com/matthewyjiang/rho/commit/0ab34fc02f304d4c6f88cfc9cfb647e6a22b8397))
* **tui:** spatial workflow graph navigation, mouse pan, and skip-edge routing ([#823](https://github.com/matthewyjiang/rho/issues/823)) ([e691235](https://github.com/matthewyjiang/rho/commit/e69123517ea35fa2fb78922e8ed6da5be671cc2f))
* **tui:** unify pickers, scroll both panes, fuzzy-match visible fields, size overlays to content ([#805](https://github.com/matthewyjiang/rho/issues/805)) ([8a60fbf](https://github.com/matthewyjiang/rho/commit/8a60fbfd6761a986263c4077a81a688d37b6172a))


### Bug Fixes

* **attach:** keep recording at high token rates ([#824](https://github.com/matthewyjiang/rho/issues/824)) ([d4fec84](https://github.com/matthewyjiang/rho/commit/d4fec8494dfd125023ef1af195fede058df9a6e4))
* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))
* **tui-pty:** simplify proof-plate palette and version pin ([#810](https://github.com/matthewyjiang/rho/issues/810)) ([8553e91](https://github.com/matthewyjiang/rho/commit/8553e915e7cd4ea0ac74a8724e8d08fc634e32f3))
* **tui:** drop duplicate paths on multi-file edit cards ([#826](https://github.com/matthewyjiang/rho/issues/826)) ([16c98e3](https://github.com/matthewyjiang/rho/commit/16c98e3c1898cbb885e71366a940efeaa52aa688))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.2 to 1.17.3
    * rho-providers bumped from 0.18.1 to 0.18.2

## [1.32.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.32.1...rho-coding-agent-v1.32.2) (2026-08-07)


### Bug Fixes

* **tui:** report generation token throughput ([#803](https://github.com/matthewyjiang/rho/issues/803)) ([4772f68](https://github.com/matthewyjiang/rho/commit/4772f68ccad3fc1edf65aa666d9a49f74bfab960))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.1 to 1.17.2
    * rho-providers bumped from 0.18.0 to 0.18.1

## [1.32.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.32.0...rho-coding-agent-v1.32.1) (2026-08-07)


### Bug Fixes

* **advisor:** stream guidance into the tool card ([#796](https://github.com/matthewyjiang/rho/issues/796)) ([465c3ed](https://github.com/matthewyjiang/rho/commit/465c3eded483699a2a0fc397af5eec75b8720a72))

## [1.32.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.31.0...rho-coding-agent-v1.32.0) (2026-08-07)


### Features

* add native MCP client and Agent Plugins ([#776](https://github.com/matthewyjiang/rho/issues/776)) ([c148fa4](https://github.com/matthewyjiang/rho/commit/c148fa4319d37190e1c084977b573d6e4fef9ce6))
* **agents:** allow pinning auth profiles on rho agents ([#781](https://github.com/matthewyjiang/rho/issues/781)) ([3e1f691](https://github.com/matthewyjiang/rho/commit/3e1f691e693dd93bf888cb5c3eb3093a7169525a))
* **tui:** render workflow dependencies as a graph ([#779](https://github.com/matthewyjiang/rho/issues/779)) ([fc8bdd9](https://github.com/matthewyjiang/rho/commit/fc8bdd9bcfc3541fca183fc14951a03ebb1a5fe5))


### Bug Fixes

* **plugins:** silence clippy failures on main ([#790](https://github.com/matthewyjiang/rho/issues/790)) ([b44151d](https://github.com/matthewyjiang/rho/commit/b44151d0d40dfbfda4bf23b49feb88d6bb0bdff3))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.0 to 1.17.1
    * rho-providers bumped from 0.17.1 to 0.18.0

## [1.31.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.30.1...rho-coding-agent-v1.31.0) (2026-08-07)


### Features

* **advisor:** add selectable advisor reasoning level ([#767](https://github.com/matthewyjiang/rho/issues/767)) ([b9eb143](https://github.com/matthewyjiang/rho/commit/b9eb1437b629aa4c7ce89ed2e8f12bea3e7d99f8))
* **tui:** fold advisor cost into session total ([#761](https://github.com/matthewyjiang/rho/issues/761)) ([2360599](https://github.com/matthewyjiang/rho/commit/23605992b408199c60cd79d9441e6aceab5bfef2))
* **tui:** render display and inline math with txm ([#770](https://github.com/matthewyjiang/rho/issues/770)) ([c681deb](https://github.com/matthewyjiang/rho/commit/c681deb6097d36dcccc5bf5f5ef05b513751f824))

## [1.30.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.30.0...rho-coding-agent-v1.30.1) (2026-08-06)


### Bug Fixes

* **providers:** source Meta Muse Spark reasoning from models.dev ([#758](https://github.com/matthewyjiang/rho/issues/758)) ([a11eea0](https://github.com/matthewyjiang/rho/commit/a11eea0756cedfd2f1bb879603355c28a9c4a037))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.17.0 to 0.17.1

## [1.30.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.29.1...rho-coding-agent-v1.30.0) (2026-08-06)


### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **providers:** add Meta Model API and collapse provider registration ([#755](https://github.com/matthewyjiang/rho/issues/755)) ([b41ef92](https://github.com/matthewyjiang/rho/commit/b41ef92dcbeeba12351df4711f4817761fda0a79))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.16.0 to 1.17.0
    * rho-providers bumped from 0.16.1 to 0.17.0

## [1.29.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.29.0...rho-coding-agent-v1.29.1) (2026-08-05)


### Bug Fixes

* **providers:** keep anthropic tool schemas typed after composition strip ([#753](https://github.com/matthewyjiang/rho/issues/753)) ([207e74c](https://github.com/matthewyjiang/rho/commit/207e74cc577d4c7c905f9bcf6b6b49e7153c9db5))
* **workflow:** raise planner RLIMIT_AS above startup VmPeak ([#749](https://github.com/matthewyjiang/rho/issues/749)) ([def515e](https://github.com/matthewyjiang/rho/commit/def515e0eea9abfaf92ee0e440b43040be5bb972))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.16.0 to 0.16.1

## [1.29.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.28.1...rho-coding-agent-v1.29.0) (2026-08-04)


### Features

* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.2 to 1.16.0
    * rho-providers bumped from 0.15.5 to 0.16.0

## [1.28.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.28.0...rho-coding-agent-v1.28.1) (2026-08-04)


### Bug Fixes

* **tui:** collapse soft-wrap break spaces at emit ([#744](https://github.com/matthewyjiang/rho/issues/744)) ([0490a4d](https://github.com/matthewyjiang/rho/commit/0490a4d5de3d73ce69abaf48391212c12ef9c1f0))
* **tui:** keep activity rail and hide Thinking... in zen mode ([#742](https://github.com/matthewyjiang/rho/issues/742)) ([d377de7](https://github.com/matthewyjiang/rho/commit/d377de7275bfc5b7abeea5079974a24f0156b019))
* **tui:** resolve dim chrome from bright black, not white ([#741](https://github.com/matthewyjiang/rho/issues/741)) ([4b41737](https://github.com/matthewyjiang/rho/commit/4b4173784da5d9d9654f2a258661d97ff4524011))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.1 to 1.15.2
    * rho-providers bumped from 0.15.4 to 0.15.5

## [1.28.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.27.1...rho-coding-agent-v1.28.0) (2026-08-03)


### Features

* **cli:** expand export formats and session browsing ([#725](https://github.com/matthewyjiang/rho/issues/725)) ([037aaf5](https://github.com/matthewyjiang/rho/commit/037aaf56ed9b2cf60e1020bd18851f8d003b7046))
* **tui:** add zen mode display policy ([#736](https://github.com/matthewyjiang/rho/issues/736)) ([0f1d4c5](https://github.com/matthewyjiang/rho/commit/0f1d4c5c809240d60af179ca5afdd96096c36580))


### Bug Fixes

* **config:** warn on bad keys and stop silent CLI auto-save ([#731](https://github.com/matthewyjiang/rho/issues/731)) ([83cfa9d](https://github.com/matthewyjiang/rho/commit/83cfa9d346b8a6da5af434ce963f6108cbe8ed62))
* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))
* **run:** keep error detail and stabilize automation contracts ([#735](https://github.com/matthewyjiang/rho/issues/735)) ([d7bb0dd](https://github.com/matthewyjiang/rho/commit/d7bb0ddcbc69703b5a8e1f9499792290f83985e3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.0 to 1.15.1
    * rho-providers bumped from 0.15.3 to 0.15.4

## [1.27.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.27.0...rho-coding-agent-v1.27.1) (2026-08-03)


### Bug Fixes

* **errors:** print user-facing errors without Debug dumps ([#728](https://github.com/matthewyjiang/rho/issues/728)) ([e8a98d7](https://github.com/matthewyjiang/rho/commit/e8a98d72e4b8e87ac9320b4efdb8cf07b3fa887f))
* **tui:** add statusline hierarchy and severity escalation ([#720](https://github.com/matthewyjiang/rho/issues/720)) ([befb91b](https://github.com/matthewyjiang/rho/commit/befb91bdb5b750166cc6d6ebe18790c648c512a8))
* **tui:** floor terminal size and keep composer visible ([#727](https://github.com/matthewyjiang/rho/issues/727)) ([392aba1](https://github.com/matthewyjiang/rho/commit/392aba18d08e499cfbcf3babc8c2d7f4510d5a74))
* **tui:** match during-turn ctrl-c quit hint to idle ([#726](https://github.com/matthewyjiang/rho/issues/726)) ([7d8e818](https://github.com/matthewyjiang/rho/commit/7d8e818af4ce1f462c586bfcee2321d1409307ed))
* **tui:** raise paste collapse threshold to 5 lines ([#729](https://github.com/matthewyjiang/rho/issues/729)) ([7267f54](https://github.com/matthewyjiang/rho/commit/7267f54fd23aabec94a48351c8218bb9795f2ee3))
* **tui:** show relative age in resume picker ([#723](https://github.com/matthewyjiang/rho/issues/723)) ([a1654d8](https://github.com/matthewyjiang/rho/commit/a1654d85e3cbf9d71b839d05834648d98cd612e7))
* **tui:** silence routine status toast noise ([#721](https://github.com/matthewyjiang/rho/issues/721)) ([1559dae](https://github.com/matthewyjiang/rho/commit/1559daeee657fe0f96bf3e23fb349273594314b2))
* **tui:** surface structured picker key hints and empty states ([#724](https://github.com/matthewyjiang/rho/issues/724)) ([3174af7](https://github.com/matthewyjiang/rho/commit/3174af7c69297a3a93c2cbc13620a51d1590f9ea))

## [1.27.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.26.1...rho-coding-agent-v1.27.0) (2026-08-02)


### Features

* **tui:** open a full-screen setup on the first launch ([#715](https://github.com/matthewyjiang/rho/issues/715)) ([19b14b5](https://github.com/matthewyjiang/rho/commit/19b14b53c5693de91de870b4ef6b2a1ca1146e42))


### Bug Fixes

* **tui:** surface status feedback as a top-right toast ([#716](https://github.com/matthewyjiang/rho/issues/716)) ([cb4e8a5](https://github.com/matthewyjiang/rho/commit/cb4e8a599203f0d2ab30e36cdbb824e9a0242b8d))

## [1.26.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.26.0...rho-coding-agent-v1.26.1) (2026-08-02)


### Bug Fixes

* **tui:** open workflow hub when workspace has no workflows ([#693](https://github.com/matthewyjiang/rho/issues/693)) ([ff8d51c](https://github.com/matthewyjiang/rho/commit/ff8d51c680ba91f8e226624aa3c37edce579e543))

## [1.26.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.25.1...rho-coding-agent-v1.26.0) (2026-08-02)


### Features

* **skills:** add built-in rho-config skill ([#685](https://github.com/matthewyjiang/rho/issues/685)) ([91abfaf](https://github.com/matthewyjiang/rho/commit/91abfaf2c3bbca4a21a9aac32ff53f30f918dfee))
* **tools:** extract pdf content with pdf-inspector ([#687](https://github.com/matthewyjiang/rho/issues/687)) ([ce92355](https://github.com/matthewyjiang/rho/commit/ce92355e74a56ab11d7f2cf35ff3d246e34310d2))
* **tui:** add /changelog for recent release notes ([#681](https://github.com/matthewyjiang/rho/issues/681)) ([4269020](https://github.com/matthewyjiang/rho/commit/4269020cc967173ab7efbc2a30a65677658baa65))
* **tui:** show provider label next to model in statusline ([#686](https://github.com/matthewyjiang/rho/issues/686)) ([274c18b](https://github.com/matthewyjiang/rho/commit/274c18b805074d9cce0c0ea95db0ec48c876ece9))
* **workflow:** add deterministic DAG workflows ([#680](https://github.com/matthewyjiang/rho/issues/680)) ([77f36f9](https://github.com/matthewyjiang/rho/commit/77f36f9cc6992f78ab59ac481a1f2dc4e00350a0))


### Bug Fixes

* **workflow:** gate RLIMIT_AS to linux so the planner runs on macOS ([#689](https://github.com/matthewyjiang/rho/issues/689)) ([dabbf99](https://github.com/matthewyjiang/rho/commit/dabbf99fe1825cd1a3966aa333d0ab759f9884c4))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.14.0 to 1.15.0
    * rho-providers bumped from 0.15.2 to 0.15.3

## [1.25.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.25.0...rho-coding-agent-v1.25.1) (2026-07-31)


### Bug Fixes

* **tui:** suppress false fast notice and preserve pasted commands ([#675](https://github.com/matthewyjiang/rho/issues/675)) ([5a354f0](https://github.com/matthewyjiang/rho/commit/5a354f04dea76de50cd784652bec4f42e2b36fb4))
* **tui:** validate dropped file attachments ([#677](https://github.com/matthewyjiang/rho/issues/677)) ([1c62a3d](https://github.com/matthewyjiang/rho/commit/1c62a3d638328bc829350f1cf32649fd58f7abcb))

## [1.25.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.24.1...rho-coding-agent-v1.25.0) (2026-07-31)


### Features

* **documents:** add bounded document extraction and attachments ([#669](https://github.com/matthewyjiang/rho/issues/669)) ([d1ec3cd](https://github.com/matthewyjiang/rho/commit/d1ec3cd5d8f5683c7b8de0047070a6029bb1ec33))
* **hooks:** add typed lifecycle hooks ([#668](https://github.com/matthewyjiang/rho/issues/668)) ([4a69c3d](https://github.com/matthewyjiang/rho/commit/4a69c3dfbc2136a8c23dff515909e23886b8651f))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.1 to 1.14.0
    * rho-providers bumped from 0.15.1 to 0.15.2

## [1.24.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.24.0...rho-coding-agent-v1.24.1) (2026-07-30)


### Bug Fixes

* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.0 to 1.13.1
    * rho-providers bumped from 0.15.0 to 0.15.1

## [1.24.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.23.0...rho-coding-agent-v1.24.0) (2026-07-30)


### Features

* **tools:** restore simple edit_file ([#658](https://github.com/matthewyjiang/rho/issues/658)) ([ffac70f](https://github.com/matthewyjiang/rho/commit/ffac70f6d58d1532a4eedefbdc99463402adbf7b))
* **tui:** stream apply_patch diff cards ([#657](https://github.com/matthewyjiang/rho/issues/657)) ([e2c932e](https://github.com/matthewyjiang/rho/commit/e2c932e377f15ddfaab1e4700aa7d6f4e8ed0417))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.14.0 to 0.15.0

## [1.23.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.22.1...rho-coding-agent-v1.23.0) (2026-07-30)


### Features

* **tools:** replace edit_file with codex-style apply_patch ([#653](https://github.com/matthewyjiang/rho/issues/653)) ([eef1555](https://github.com/matthewyjiang/rho/commit/eef155521c5492b9c7f34507e82b6b7f46b896a8))
* **web:** prefer hosted search with backup provider config ([#649](https://github.com/matthewyjiang/rho/issues/649)) ([2e136e9](https://github.com/matthewyjiang/rho/commit/2e136e9025ebc318f7fac5da8e45a3134d785430))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* **sessions:** generate titles after first turn ([#652](https://github.com/matthewyjiang/rho/issues/652)) ([4b55d26](https://github.com/matthewyjiang/rho/commit/4b55d26ca0330aba4c2ec024c37e66ade3aa7bb3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.2 to 1.13.0
    * rho-providers bumped from 0.13.3 to 0.14.0

## [1.22.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.22.0...rho-coding-agent-v1.22.1) (2026-07-29)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.13.2 to 0.13.3

## [1.22.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.21.0...rho-coding-agent-v1.22.0) (2026-07-29)


### Features

* **sessions:** add workspace rewind checkpoints ([#638](https://github.com/matthewyjiang/rho/issues/638)) ([5a90b2d](https://github.com/matthewyjiang/rho/commit/5a90b2db5b1170f2701cbac1c0c7d056f9158754))


### Bug Fixes

* **tui:** open approval prompts at the start and default to deny ([#636](https://github.com/matthewyjiang/rho/issues/636)) ([59efc07](https://github.com/matthewyjiang/rho/commit/59efc07b26bf67597ebbe05551cd22f3affedc96))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.1 to 1.12.2
    * rho-providers bumped from 0.13.1 to 0.13.2

## [1.21.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.20.0...rho-coding-agent-v1.21.0) (2026-07-28)


### Features

* **tui:** add in-app editor for user-defined agents ([#630](https://github.com/matthewyjiang/rho/issues/630)) ([7243fcc](https://github.com/matthewyjiang/rho/commit/7243fcce4c7261d38547a9e3c9e5a51aea7e9db0))


### Bug Fixes

* **metrics:** include reasoning latency in output rate ([#632](https://github.com/matthewyjiang/rho/issues/632)) ([7f7fa39](https://github.com/matthewyjiang/rho/commit/7f7fa39d88e3032a4433a105e07a785989406944))
* **tui:** bound inline shell output and size pickers to the terminal ([#634](https://github.com/matthewyjiang/rho/issues/634)) ([3535df4](https://github.com/matthewyjiang/rho/commit/3535df45f9168a75b75bee60c0c16165fb923b2b))
* **tui:** live drag selection highlight and screen-wide text copy ([#633](https://github.com/matthewyjiang/rho/issues/633)) ([71e6a35](https://github.com/matthewyjiang/rho/commit/71e6a35279ea63885be460124b262c4ce4339a86))


### Performance Improvements

* **tui,session:** cut truncation allocations and cache tree branch count ([#629](https://github.com/matthewyjiang/rho/issues/629)) ([a1c53f0](https://github.com/matthewyjiang/rho/commit/a1c53f0894b20268a96f17a770291ba2a2c31eaa))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.0 to 1.12.1
    * rho-providers bumped from 0.13.0 to 0.13.1

## [1.20.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.19.2...rho-coding-agent-v1.20.0) (2026-07-28)


### Features

* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.11.0 to 1.12.0
    * rho-providers bumped from 0.12.2 to 0.13.0

## [1.19.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.19.1...rho-coding-agent-v1.19.2) (2026-07-28)


### Bug Fixes

* **auth:** prefer credential-backed auth and fix Ollama Cloud reasoning ([#619](https://github.com/matthewyjiang/rho/issues/619)) ([1a57f6f](https://github.com/matthewyjiang/rho/commit/1a57f6f24292b63a6ba4ba314843c2fb308792cf))
* **auth:** restore ollama device test key dir on unwind and isolate temp dir ([#621](https://github.com/matthewyjiang/rho/issues/621)) ([d2a345d](https://github.com/matthewyjiang/rho/commit/d2a345df56b1ae815007829a04cc21172500530d))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.12.1 to 0.12.2

## [1.19.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.19.0...rho-coding-agent-v1.19.1) (2026-07-28)


### Bug Fixes

* **auth:** stop waiting for Ollama device callback ([#616](https://github.com/matthewyjiang/rho/issues/616)) ([54288d2](https://github.com/matthewyjiang/rho/commit/54288d28f7bcc68a36f0424e5de6c28e470fb479))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.12.0 to 0.12.1

## [1.19.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.18.2...rho-coding-agent-v1.19.0) (2026-07-28)


### Features

* **auth:** add active auth mode switcher ([#609](https://github.com/matthewyjiang/rho/issues/609)) ([a2b0f68](https://github.com/matthewyjiang/rho/commit/a2b0f68f71033ca6f8594a35368a79b2388916ca))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **providers:** add Ollama Cloud API provider ([#597](https://github.com/matthewyjiang/rho/issues/597)) ([f6a62dd](https://github.com/matthewyjiang/rho/commit/f6a62ddb8c77bae1f6ba386328b79db625ec1e5d))
* **tui:** add composer prompt marker ([#614](https://github.com/matthewyjiang/rho/issues/614)) ([1994ff5](https://github.com/matthewyjiang/rho/commit/1994ff5280bf60fc4594d4a602fa8ab60c88f052))


### Bug Fixes

* **agents:** clarify foreground agent batch behavior ([#606](https://github.com/matthewyjiang/rho/issues/606)) ([9574e48](https://github.com/matthewyjiang/rho/commit/9574e4836a3c6e14eb28bc5863b8d2abc334e140))
* **tui:** keep markdown list markers with long tokens ([#611](https://github.com/matthewyjiang/rho/issues/611)) ([90dafc9](https://github.com/matthewyjiang/rho/commit/90dafc9901cc75eefcf096011c463f6e7d9043d0))
* **tui:** preserve newlines in multi-line tool header wrap ([#608](https://github.com/matthewyjiang/rho/issues/608)) ([1194088](https://github.com/matthewyjiang/rho/commit/11940882591bc4b2f752ae9456e9d1bb4de77e53))
* **tui:** render command suggestions above composer ([#612](https://github.com/matthewyjiang/rho/issues/612)) ([9a81647](https://github.com/matthewyjiang/rho/commit/9a81647aace42ea97b461dcd17112dbd3f1a8459))
* **tui:** show help shortcut descriptions ([#613](https://github.com/matthewyjiang/rho/issues/613)) ([3e8a034](https://github.com/matthewyjiang/rho/commit/3e8a034385e28c9cbc48ac973167eba9a92a9579))


### Performance Improvements

* optimize orchestration and session hot paths ([#603](https://github.com/matthewyjiang/rho/issues/603)) ([62aa8f5](https://github.com/matthewyjiang/rho/commit/62aa8f50358fc82f1e6bef5bf0d2348fc6c0aaac))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.3 to 1.11.0
    * rho-providers bumped from 0.11.1 to 0.12.0

## [1.18.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.18.1...rho-coding-agent-v1.18.2) (2026-07-27)


### Bug Fixes

* **tui:** keep stable prose while stream emphasis completes ([#598](https://github.com/matthewyjiang/rho/issues/598)) ([2f61fb0](https://github.com/matthewyjiang/rho/commit/2f61fb044939dcb86ede9db205bfe7e94e957d42))

## [1.18.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.18.0...rho-coding-agent-v1.18.1) (2026-07-27)


### Bug Fixes

* **tui:** keep cwd basename visible in status line ([#591](https://github.com/matthewyjiang/rho/issues/591)) ([3390a4d](https://github.com/matthewyjiang/rho/commit/3390a4d6ad8954d2c4bcc712cff664777c7aaa36))
* **tui:** stabilize live stream tool cards and markdown previews ([#595](https://github.com/matthewyjiang/rho/issues/595)) ([752794f](https://github.com/matthewyjiang/rho/commit/752794f407d65533e97924d6e89ceeba443886c0))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.2 to 1.10.3
    * rho-providers bumped from 0.11.0 to 0.11.1

## [1.18.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.17.1...rho-coding-agent-v1.18.0) (2026-07-27)


### Features

* **tui:** unify tool transcript cards as Call + Children ([#586](https://github.com/matthewyjiang/rho/issues/586)) ([ce52cdd](https://github.com/matthewyjiang/rho/commit/ce52cddb6dbf0ac1b2878b6f3bd468a87547f8fa))


### Bug Fixes

* **tui:** smooth streamed text and hold bare tool previews ([#590](https://github.com/matthewyjiang/rho/issues/590)) ([591d271](https://github.com/matthewyjiang/rho/commit/591d2715b04a4e836648b2e7a4db1dc41cf5119b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.10.2 to 0.11.0

## [1.17.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.17.0...rho-coding-agent-v1.17.1) (2026-07-27)


### Bug Fixes

* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **tools:** pin fetch connections to SSRF-vetted addresses ([#572](https://github.com/matthewyjiang/rho/issues/572)) ([45cdd40](https://github.com/matthewyjiang/rho/commit/45cdd40b144fb5f5045bf28bce0c712e949750f1)), closes [#525](https://github.com/matthewyjiang/rho/issues/525)
* **tui:** adapt reasoning contrast to terminal palette ([#582](https://github.com/matthewyjiang/rho/issues/582)) ([5361522](https://github.com/matthewyjiang/rho/commit/53615229f2a7c160763757636b48c9e5d7d72526))
* **tui:** lighten overlay chrome and fix picker polish ([#581](https://github.com/matthewyjiang/rho/issues/581)) ([1ee1cdf](https://github.com/matthewyjiang/rho/commit/1ee1cdfcfbe72c6d74436d4a4ee356af2c6cf151))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.1 to 1.10.2
    * rho-providers bumped from 0.10.1 to 0.10.2

## [1.17.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.16.0...rho-coding-agent-v1.17.0) (2026-07-26)


### Features

* **sessions:** delete sessions and cascade parent-linked runs ([#563](https://github.com/matthewyjiang/rho/issues/563)) ([ded020d](https://github.com/matthewyjiang/rho/commit/ded020d6763c2d078ed245b4ead2bd5c1790c394))
* **sessions:** nest subagent artifacts with parent sessions ([#567](https://github.com/matthewyjiang/rho/issues/567)) ([3edf433](https://github.com/matthewyjiang/rho/commit/3edf433d691e1d5f6d3525334fade75025349d32))


### Bug Fixes

* **prompt:** guide agents to use supported mermaid diagrams ([#555](https://github.com/matthewyjiang/rho/issues/555)) ([a92c928](https://github.com/matthewyjiang/rho/commit/a92c928b684fc54fdb89152ba395b972e33fb45e))
* **tui:** render narrow mermaid flowcharts and explain fallbacks ([#565](https://github.com/matthewyjiang/rho/issues/565)) ([0bf7ad7](https://github.com/matthewyjiang/rho/commit/0bf7ad719fa32d00bc6d1bc7857307032fd9f1f6))
* **tui:** reuse tool stream previews and allow codex parallel tools ([#566](https://github.com/matthewyjiang/rho/issues/566)) ([fa0074a](https://github.com/matthewyjiang/rho/commit/fa0074ae125972ac533ae09b30915f7e479674bd))
* **tui:** share mermaid fan-out source stems ([#562](https://github.com/matthewyjiang/rho/issues/562)) ([e244b45](https://github.com/matthewyjiang/rho/commit/e244b45ddae6a0f227cb4003b6e809342dbb696c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.0 to 1.10.1
    * rho-providers bumped from 0.10.0 to 0.10.1

## [1.16.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.15.0...rho-coding-agent-v1.16.0) (2026-07-26)


### Features

* **tools:** add in-process grep and glob workspace tools ([#554](https://github.com/matthewyjiang/rho/issues/554)) ([e422a99](https://github.com/matthewyjiang/rho/commit/e422a990332afff330b096d8960d4e0fa07a5838))
* **tui:** open or copy subagent attach from the activity rail ([#552](https://github.com/matthewyjiang/rho/issues/552)) ([95e4ca6](https://github.com/matthewyjiang/rho/commit/95e4ca698ab92f858587356ba16e29476bcfd972))


### Bug Fixes

* **tui:** report missing clipboard image helpers clearly ([#549](https://github.com/matthewyjiang/rho/issues/549)) ([774e965](https://github.com/matthewyjiang/rho/commit/774e965e851ca622197ffb0e5469d69db270967a))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.9.0 to 0.10.0

## [1.15.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.14.0...rho-coding-agent-v1.15.0) (2026-07-25)


### Features

* **agents:** add Claude Code subagent runtime ([#541](https://github.com/matthewyjiang/rho/issues/541)) ([c1385ec](https://github.com/matthewyjiang/rho/commit/c1385ecae9b2eb967ae73ecc09c20cc80bc63479))
* **providers:** add xAI server-side context compaction ([#542](https://github.com/matthewyjiang/rho/issues/542)) ([2d43f13](https://github.com/matthewyjiang/rho/commit/2d43f134669414b3d9b7332a4c9d17aaa1346d9f))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tui:** include subagent costs in status and info ([#548](https://github.com/matthewyjiang/rho/issues/548)) ([9517f00](https://github.com/matthewyjiang/rho/commit/9517f0012dd2001213fd10294287f8a0739e5d2c))


### Bug Fixes

* **errors:** surface failures that were silently swallowed ([#546](https://github.com/matthewyjiang/rho/issues/546)) ([1d4eee3](https://github.com/matthewyjiang/rho/commit/1d4eee3ea2e45d459897198d48babbe3ded3bf19))
* **subagents:** run multi-agent batches in parallel ([#544](https://github.com/matthewyjiang/rho/issues/544)) ([7dd6706](https://github.com/matthewyjiang/rho/commit/7dd6706f6aade0a45d70336a42d259b4a3c12a4f))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))
* **tui:** correct attach token breakdown and add scrollbar ([#550](https://github.com/matthewyjiang/rho/issues/550)) ([fbe18e7](https://github.com/matthewyjiang/rho/commit/fbe18e750d180ab911ce85f12301735cea6246e1))
* **tui:** stream live agent tool-call prompt previews ([#543](https://github.com/matthewyjiang/rho/issues/543)) ([bca0596](https://github.com/matthewyjiang/rho/commit/bca059632b80b87708e4a32f66c7a77375f3ad3d))
* **update:** fetch the install script from the release tag, not main ([#536](https://github.com/matthewyjiang/rho/issues/536)) ([332401a](https://github.com/matthewyjiang/rho/commit/332401a5d14e93d1b609c5a667aa45e9d4276bb1)), closes [#497](https://github.com/matthewyjiang/rho/issues/497)
* **web:** keep github tokens out of git argv and harden fetch targets ([#547](https://github.com/matthewyjiang/rho/issues/547)) ([7b32573](https://github.com/matthewyjiang/rho/commit/7b3257399ea75223531859323142a17c6d000500))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.9.0 to 1.10.0
    * rho-providers bumped from 0.8.1 to 0.9.0

## [1.14.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.13.0...rho-coding-agent-v1.14.0) (2026-07-24)


### Features

* **questionnaire:** add focused default selection ([#530](https://github.com/matthewyjiang/rho/issues/530)) ([47d1853](https://github.com/matthewyjiang/rho/commit/47d185377a68881c96db18ee186b3173215e39ee))


### Bug Fixes

* **sessions:** resolve sessions by id from any directory ([#492](https://github.com/matthewyjiang/rho/issues/492)) ([f88ff1b](https://github.com/matthewyjiang/rho/commit/f88ff1b38cecbfebbc1046be14f9e0964c45d364))
* **tui:** omit trailing blank on open stream entries ([#531](https://github.com/matthewyjiang/rho/issues/531)) ([5bb0046](https://github.com/matthewyjiang/rho/commit/5bb0046c93bca07e80a7dffcfb3b1bf46a701bbb))
* **tui:** smooth external editor terminal handoff ([#527](https://github.com/matthewyjiang/rho/issues/527)) ([387e355](https://github.com/matthewyjiang/rho/commit/387e355e1ce049df1920ff96147b9108e9c33f52))
* **tui:** warn when external editor is unset ([#533](https://github.com/matthewyjiang/rho/issues/533)) ([fa556be](https://github.com/matthewyjiang/rho/commit/fa556be42935b8f64f651dba15c20d2d994f90be))
* **web:** inline fetch content and session-folder web sidecars ([#532](https://github.com/matthewyjiang/rho/issues/532)) ([76e2f45](https://github.com/matthewyjiang/rho/commit/76e2f4534fd5618fffbebd0861089654a234f181))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.8.0 to 1.9.0
    * rho-providers bumped from 0.8.0 to 0.8.1

## [1.13.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.12.2...rho-coding-agent-v1.13.0) (2026-07-23)


### Features

* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))
* **tui:** add help overlay and clarify shell mode ([#515](https://github.com/matthewyjiang/rho/issues/515)) ([f1e4d20](https://github.com/matthewyjiang/rho/commit/f1e4d201ee559c385413d50b8b7eb451fb298815))
* **tui:** add model handoff compaction choice ([#513](https://github.com/matthewyjiang/rho/issues/513)) ([c7bdf84](https://github.com/matthewyjiang/rho/commit/c7bdf8439672dc4f741db3ab505accad1ff2dac1))
* **tui:** edit composer in external editor ([#523](https://github.com/matthewyjiang/rho/issues/523)) ([113e8c4](https://github.com/matthewyjiang/rho/commit/113e8c43929b1981d9d9a00ed97873c1db4628cd))


### Bug Fixes

* **usage:** normalize cache write token accounting ([#511](https://github.com/matthewyjiang/rho/issues/511)) ([4e15982](https://github.com/matthewyjiang/rho/commit/4e15982a1e6f4738d40611d77c721ac26051bfda))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.2 to 1.8.0
    * rho-providers bumped from 0.7.1 to 0.8.0

## [1.12.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.12.1...rho-coding-agent-v1.12.2) (2026-07-22)


### Bug Fixes

* **tui:** paste image paths and fall back kitty under herdr ([#504](https://github.com/matthewyjiang/rho/issues/504)) ([c140bfe](https://github.com/matthewyjiang/rho/commit/c140bfe6994f4ffc42756075ec801eff6e63ce40))
* **tui:** skip modifyOtherKeys on Windows under ConPTY ([#507](https://github.com/matthewyjiang/rho/issues/507)) ([2bb3b49](https://github.com/matthewyjiang/rho/commit/2bb3b49f2435714dc010ba6fe21cb088acd8f115))

## [1.12.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.12.0...rho-coding-agent-v1.12.1) (2026-07-22)


### Bug Fixes

* **tools:** block private fetch targets with resolve-then-check SSRF guard ([#499](https://github.com/matthewyjiang/rho/issues/499)) ([5dbc11c](https://github.com/matthewyjiang/rho/commit/5dbc11c9f8692d9d46f9c1008b87de054c9fb3df))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** report herdr blocked during questionnaire ([#500](https://github.com/matthewyjiang/rho/issues/500)) ([1dcdbe9](https://github.com/matthewyjiang/rho/commit/1dcdbe9e86bb61cf0cd55a705c28941d9dfcb241))
* **tui:** sort slash commands and provider pickers alphabetically ([#498](https://github.com/matthewyjiang/rho/issues/498)) ([0e2c16c](https://github.com/matthewyjiang/rho/commit/0e2c16cd9b5ac6b5c9c28259a09c0428f64a72ab))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.1 to 1.7.2
    * rho-providers bumped from 0.7.0 to 0.7.1

## [1.12.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.11.0...rho-coding-agent-v1.12.0) (2026-07-22)


### Features

* **auth:** choose credential store on first login ([#487](https://github.com/matthewyjiang/rho/issues/487)) ([d2e0cb7](https://github.com/matthewyjiang/rho/commit/d2e0cb75a32a7789df2010ad8eee7b96659dc105))
* **providers:** add Poolside API platform ([#483](https://github.com/matthewyjiang/rho/issues/483)) ([4684de7](https://github.com/matthewyjiang/rho/commit/4684de700f4312a90fa6d3173343a1dcfe7ef44d))
* **tui:** show basic controls under session header ([#485](https://github.com/matthewyjiang/rho/issues/485)) ([fb65596](https://github.com/matthewyjiang/rho/commit/fb65596fc06ca4f3d0312dce91d04465df80e34b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.0 to 1.7.1
    * rho-providers bumped from 0.6.0 to 0.7.0

## [1.11.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.10.0...rho-coding-agent-v1.11.0) (2026-07-22)


### Features

* **auth:** add configurable credential storage ([#478](https://github.com/matthewyjiang/rho/issues/478)) ([e778eda](https://github.com/matthewyjiang/rho/commit/e778edab71ec7e3c2f21137760f53bd0b8089469))
* **auth:** add OpenRouter OAuth login ([#472](https://github.com/matthewyjiang/rho/issues/472)) ([42af8e7](https://github.com/matthewyjiang/rho/commit/42af8e7a95bc1d16245f89dd1ebe74e6c4f56b7b))
* **cli:** add structured run output ([#467](https://github.com/matthewyjiang/rho/issues/467)) ([c4088bb](https://github.com/matthewyjiang/rho/commit/c4088bb03ef0e7e1b69de5e671773399755fe07b))
* **models:** resolve aliases in model command ([#463](https://github.com/matthewyjiang/rho/issues/463)) ([a74ee4d](https://github.com/matthewyjiang/rho/commit/a74ee4d6f01e544e077cbe6f8da8c3eeeca305c1))
* **providers:** add Ollama support ([#466](https://github.com/matthewyjiang/rho/issues/466)) ([3a5a6d2](https://github.com/matthewyjiang/rho/commit/3a5a6d2fbf9fddcd87fbbb996e22438436a87823))
* **sessions:** add conversation tree navigation ([#474](https://github.com/matthewyjiang/rho/issues/474)) ([8abb138](https://github.com/matthewyjiang/rho/commit/8abb1387a0c96dcf3142166d27b4db108d1c5181))
* **tui:** add navigable agent popup ([#473](https://github.com/matthewyjiang/rho/issues/473)) ([f2d8726](https://github.com/matthewyjiang/rho/commit/f2d87260962a3a34f71b8fe886ffbb37fe30c7c8))
* **tui:** show context tokens with compact K/M units ([#477](https://github.com/matthewyjiang/rho/issues/477)) ([52ec30d](https://github.com/matthewyjiang/rho/commit/52ec30d6328ac0a11b3412511aa0c2142c375c1f))
* **tui:** show conversation tree in overlay popup ([#480](https://github.com/matthewyjiang/rho/issues/480)) ([635763d](https://github.com/matthewyjiang/rho/commit/635763d69b7a455494101d394819c79704c20873))
* **tui:** show thought duration after reasoning ([#479](https://github.com/matthewyjiang/rho/issues/479)) ([7776f5a](https://github.com/matthewyjiang/rho/commit/7776f5aa0e126d74f8beff8b1540fb3b214c38ee))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.6.0 to 1.7.0
    * rho-providers bumped from 0.5.0 to 0.6.0

## [1.10.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.9.0...rho-coding-agent-v1.10.0) (2026-07-21)


### Features

* **agent:** add configurable built-in roles ([#447](https://github.com/matthewyjiang/rho/issues/447)) ([c139079](https://github.com/matthewyjiang/rho/commit/c1390794b8aa0e2a91ae1830cd284fdc6e2931e5))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **tui:** redesign config as category browser ([#441](https://github.com/matthewyjiang/rho/issues/441)) ([be6efe7](https://github.com/matthewyjiang/rho/commit/be6efe70f88adf492959ac1e51952f863c196dc2))


### Bug Fixes

* **clipboard:** prioritize nonzero child exit status ([#446](https://github.com/matthewyjiang/rho/issues/446)) ([3e98c52](https://github.com/matthewyjiang/rho/commit/3e98c528d74096ad27bdc7d9b267bcc9872c8511))
* **clipboard:** resolve helpers on PATH instead of executing them to probe ([#440](https://github.com/matthewyjiang/rho/issues/440)) ([50b84c7](https://github.com/matthewyjiang/rho/commit/50b84c76bcb35171f8d8d027a2c60cc950968697))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **skills:** execute slash skill invocations ([#452](https://github.com/matthewyjiang/rho/issues/452)) ([49480a8](https://github.com/matthewyjiang/rho/commit/49480a8397ab21fa46af7746cae67f036e3d9e1b))
* **tui:** add slash commands to input history ([#443](https://github.com/matthewyjiang/rho/issues/443)) ([1050a5b](https://github.com/matthewyjiang/rho/commit/1050a5bb65e10265a0d44ef37e7fbf3f4275d553))
* **tui:** clarify permission and goal status ([#444](https://github.com/matthewyjiang/rho/issues/444)) ([2a43c59](https://github.com/matthewyjiang/rho/commit/2a43c5907283b316f5af4ab5ca3ad933c6b13b1e))
* **tui:** simplify responsive status details ([#458](https://github.com/matthewyjiang/rho/issues/458)) ([c4a1af2](https://github.com/matthewyjiang/rho/commit/c4a1af2185e5dc7e14746e39359160063600e982))
* **tui:** wait for delegated goal work ([#457](https://github.com/matthewyjiang/rho/issues/457)) ([fc6087d](https://github.com/matthewyjiang/rho/commit/fc6087d4dfcbba2f3b82c5c9c0387dc31a59ab0b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.5.0 to 1.6.0
    * rho-providers bumped from 0.4.0 to 0.5.0

## [1.9.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.8.2...rho-coding-agent-v1.9.0) (2026-07-20)


### Features

* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))
* **tui:** render markdown image previews ([#434](https://github.com/matthewyjiang/rho/issues/434)) ([bb1f857](https://github.com/matthewyjiang/rho/commit/bb1f857fc4c826b8489d4765dfb3c5c51a3bce05))


### Bug Fixes

* **tools:** correct ToolError path in web fetch tests ([#433](https://github.com/matthewyjiang/rho/issues/433)) ([814d22c](https://github.com/matthewyjiang/rho/commit/814d22c5bf94d60ddd5a9cb9f5973cd6e1e92965))
* **tools:** keep truncated web fetches when the byte cap splits a character ([#339](https://github.com/matthewyjiang/rho/issues/339)) ([f762a88](https://github.com/matthewyjiang/rho/commit/f762a88d2999651e22f53900782ff431947627db))
* **tui:** use native clipboard APIs before OSC 52 ([#416](https://github.com/matthewyjiang/rho/issues/416)) ([3dee5bb](https://github.com/matthewyjiang/rho/commit/3dee5bba112a8a8ce7c6389458e194a432e5e911))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.4.0 to 1.5.0
    * rho-providers bumped from 0.3.2 to 0.4.0

## [1.8.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.8.1...rho-coding-agent-v1.8.2) (2026-07-20)


### Bug Fixes

* **release:** align dependent tool versions ([#426](https://github.com/matthewyjiang/rho/issues/426)) ([7b9ea52](https://github.com/matthewyjiang/rho/commit/7b9ea5211419bd600000466a0aab2d3d0405cda8))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.3.1 to 0.3.2

## [1.8.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.8.0...rho-coding-agent-v1.8.1) (2026-07-20)


### Bug Fixes

* **agents:** batch completion notifications and acknowledge observed results ([#422](https://github.com/matthewyjiang/rho/issues/422)) ([1169791](https://github.com/matthewyjiang/rho/commit/11697919d3149581a40bf0b7dc02950c0afe4b13))
* **tui:** improve agent tool displays ([#413](https://github.com/matthewyjiang/rho/issues/413)) ([062edd0](https://github.com/matthewyjiang/rho/commit/062edd0851848c4fbd7754b47ec5dd588605989f))
* **tui:** isolate test config from user settings ([#419](https://github.com/matthewyjiang/rho/issues/419)) ([274df7f](https://github.com/matthewyjiang/rho/commit/274df7fa121b677f76ee2f36ddb33934c6e0ef32))
* **tui:** use stable braille spinner ([#423](https://github.com/matthewyjiang/rho/issues/423)) ([de62286](https://github.com/matthewyjiang/rho/commit/de622866c76987cddbb653c625f92797a3aff4aa))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.3.0 to 0.3.1

## [1.8.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.7.0...rho-coding-agent-v1.8.0) (2026-07-20)


### Features

* **agents:** move background-run contract into tool descriptions ([#405](https://github.com/matthewyjiang/rho/issues/405)) ([b75d0fa](https://github.com/matthewyjiang/rho/commit/b75d0fac659cd85a5469ce962e2bd026c673e288))
* **config:** user-defined model aliases so pinned models live in one place ([#404](https://github.com/matthewyjiang/rho/issues/404)) ([09dff65](https://github.com/matthewyjiang/rho/commit/09dff65d81788feeb2493ad971a9b5aff8fcb2c4))
* **tui:** show detailed activity stages ([#403](https://github.com/matthewyjiang/rho/issues/403)) ([267a47b](https://github.com/matthewyjiang/rho/commit/267a47bd3894f7a6ed64c82d98920a2c2d585e91))


### Bug Fixes

* **agents:** yield while background work completes ([#396](https://github.com/matthewyjiang/rho/issues/396)) ([d54e9f3](https://github.com/matthewyjiang/rho/commit/d54e9f34d794f33bb493a3f0077582c6d37c4148))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.3.0 to 1.4.0
    * rho-providers bumped from 0.2.1 to 0.3.0

## [1.7.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.6.1...rho-coding-agent-v1.7.0) (2026-07-18)


### Features

* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))


### Bug Fixes

* **ci:** sync released tool dependency versions ([#391](https://github.com/matthewyjiang/rho/issues/391)) ([fc78948](https://github.com/matthewyjiang/rho/commit/fc78948953a790dcf6a8f783e67748cae0dd61dc))
* **tui:** render markdown in reasoning traces ([#394](https://github.com/matthewyjiang/rho/issues/394)) ([3a88542](https://github.com/matthewyjiang/rho/commit/3a88542608fc383e6de12ea29b193e071a83f824))
* **tui:** show shell prompt during tool streaming ([#390](https://github.com/matthewyjiang/rho/issues/390)) ([6e6b39d](https://github.com/matthewyjiang/rho/commit/6e6b39ddaf2b7337e12bb5d7a11bce97d971f10c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.2.0 to 1.3.0
    * rho-providers bumped from 0.2.0 to 0.2.1

## [1.6.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.6.0...rho-coding-agent-v1.6.1) (2026-07-18)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-providers bumped from 0.1.0 to 0.2.0

## [1.6.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.5.0...rho-coding-agent-v1.6.0) (2026-07-17)


### Features

* **tools:** add permission modes ([#372](https://github.com/matthewyjiang/rho/issues/372)) ([dd45f31](https://github.com/matthewyjiang/rho/commit/dd45f3161d53042baaed679fb101cacd929417a1))
* **tui:** move persistent commands into config ([#377](https://github.com/matthewyjiang/rho/issues/377)) ([9e58048](https://github.com/matthewyjiang/rho/commit/9e58048486633a191879bbbfdfe5027027c37d07))
* **tui:** preview custom agent prompts ([#378](https://github.com/matthewyjiang/rho/issues/378)) ([235eed1](https://github.com/matthewyjiang/rho/commit/235eed1636685e4fbe683a9d00906ee5554a408a))
* **tui:** redesign questionnaire with tabbed question layout ([#369](https://github.com/matthewyjiang/rho/issues/369)) ([a90135a](https://github.com/matthewyjiang/rho/commit/a90135a494409cfc1c99ffd2226bee9075788d41))
* **usage:** add durable request ledger ([#381](https://github.com/matthewyjiang/rho/issues/381)) ([0502b99](https://github.com/matthewyjiang/rho/commit/0502b9987be74a8922f675ab941eadb23bc88b12))


### Bug Fixes

* **agents:** reserve background results for notifications ([#384](https://github.com/matthewyjiang/rho/issues/384)) ([377e35e](https://github.com/matthewyjiang/rho/commit/377e35effa871595a188d6c92a078599e757a215))
* **prompt:** require fenced Mermaid diagrams ([#375](https://github.com/matthewyjiang/rho/issues/375)) ([afd5656](https://github.com/matthewyjiang/rho/commit/afd56567c43779b50ad13999b78325d5eb834d31))
* **provider:** classify Anthropic in-stream provider errors by type ([#345](https://github.com/matthewyjiang/rho/issues/345)) ([768562d](https://github.com/matthewyjiang/rho/commit/768562d789b16558be24d407df325bbdd0204c95)), closes [#343](https://github.com/matthewyjiang/rho/issues/343)
* **tui:** align mouse selection with history viewport ([#370](https://github.com/matthewyjiang/rho/issues/370)) ([d91ff50](https://github.com/matthewyjiang/rho/commit/d91ff50ceb50723c64b263f9ce6caef04b6ad0ea))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.1.0 to 1.2.0

## [1.5.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.4.1...rho-coding-agent-v1.5.0) (2026-07-17)


### Features

* **agents:** unify agent definitions and execution ([#355](https://github.com/matthewyjiang/rho/issues/355)) ([157712e](https://github.com/matthewyjiang/rho/commit/157712e86906b72c5c76fe3664380700cb37eb7f))
* **auth:** add grouped login methods and xAI API keys ([#363](https://github.com/matthewyjiang/rho/issues/363)) ([1f1fdc9](https://github.com/matthewyjiang/rho/commit/1f1fdc93b5c3476ade353801ef9fc34d33c50897))
* **providers:** add Moonshot and Kimi authentication ([#359](https://github.com/matthewyjiang/rho/issues/359)) ([051fbda](https://github.com/matthewyjiang/rho/commit/051fbda5153c0c1d3dd8fd0a8d12c97c1b8f66ea))
* **providers:** add OpenRouter API key support ([#365](https://github.com/matthewyjiang/rho/issues/365)) ([51c69dd](https://github.com/matthewyjiang/rho/commit/51c69ddcca60d21da189d23ff91858bcc16f9242))
* **tui:** add agent browser and creator skill ([#366](https://github.com/matthewyjiang/rho/issues/366)) ([84937e3](https://github.com/matthewyjiang/rho/commit/84937e343f3ae8e1a9489b31ce9366231aea527b))
* **tui:** add Kimi OAuth usage limits ([#361](https://github.com/matthewyjiang/rho/issues/361)) ([84a8ca8](https://github.com/matthewyjiang/rho/commit/84a8ca8ca9c5760304eb3b789220e2a76e03a302))


### Bug Fixes

* **tui:** constrain activity rail background ([#362](https://github.com/matthewyjiang/rho/issues/362)) ([139b9e1](https://github.com/matthewyjiang/rho/commit/139b9e131ed3022d78aebf8be603be772cb7cfda))
* **tui:** extend activity background to agents ([#364](https://github.com/matthewyjiang/rho/issues/364)) ([11cfc9e](https://github.com/matthewyjiang/rho/commit/11cfc9ef5dd752a851a64739571e22c5767ffe94))

## [1.4.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.4.0...rho-coding-agent-v1.4.1) (2026-07-17)


### Bug Fixes

* **packaging:** enable test fixtures in Arch build ([#356](https://github.com/matthewyjiang/rho/issues/356)) ([2e19b5e](https://github.com/matthewyjiang/rho/commit/2e19b5e0e6bdbd7881ec2b5f0becc4ae5adb596f))
* **tui:** extend activity background below spinner ([#354](https://github.com/matthewyjiang/rho/issues/354)) ([d692736](https://github.com/matthewyjiang/rho/commit/d69273600b1a91737ec1217d5f007d22337985e8))

## [1.4.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.3.0...rho-coding-agent-v1.4.0) (2026-07-17)


### Features

* **tui:** render Mermaid diagrams in transcripts ([#348](https://github.com/matthewyjiang/rho/issues/348)) ([f91c478](https://github.com/matthewyjiang/rho/commit/f91c478b339534cc282b7d12615ff91d9af29d30))


### Bug Fixes

* **subagents:** improve background delegation flow ([#351](https://github.com/matthewyjiang/rho/issues/351)) ([80d0aff](https://github.com/matthewyjiang/rho/commit/80d0aff044d266295f80ba490b471018fb0e4af1))
* **tui:** preserve selection and delete paste markers atomically ([#352](https://github.com/matthewyjiang/rho/issues/352)) ([c1e699e](https://github.com/matthewyjiang/rho/commit/c1e699ed4da860dc4de7ea3cc6dce4e16234854f))
* **tui:** unify activity rail background ([#349](https://github.com/matthewyjiang/rho/issues/349)) ([a16f24e](https://github.com/matthewyjiang/rho/commit/a16f24e5e62629ca4ec8aafc842b6d27b0c947cc))

## [1.3.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.2.0...rho-coding-agent-v1.3.0) (2026-07-16)


### Features

* **subagents:** add read-only attachment ([#333](https://github.com/matthewyjiang/rho/issues/333)) ([e63340c](https://github.com/matthewyjiang/rho/commit/e63340ccb81dc501e483f3036871e688c63b3595))
* **tui:** add retractable pending input ([#334](https://github.com/matthewyjiang/rho/issues/334)) ([5f293a2](https://github.com/matthewyjiang/rho/commit/5f293a2221e0dcd5457eccbc8675eed2463d878e))
* **tui:** show running subagents ([#340](https://github.com/matthewyjiang/rho/issues/340)) ([5523a90](https://github.com/matthewyjiang/rho/commit/5523a907e2a8b87e910baea8a03c75a0173af9e2))


### Bug Fixes

* **providers:** show bounded error diagnostics ([#344](https://github.com/matthewyjiang/rho/issues/344)) ([e3fc489](https://github.com/matthewyjiang/rho/commit/e3fc48984590d34e19238e157e2479fa3c9d0d20))
* **release:** auto-merge Scoop manifest updates ([#329](https://github.com/matthewyjiang/rho/issues/329)) ([1aaf368](https://github.com/matthewyjiang/rho/commit/1aaf36827323856d96295aba958e02e5239937d4))
* **subagents:** discourage unnecessary delegation ([#332](https://github.com/matthewyjiang/rho/issues/332)) ([489a43f](https://github.com/matthewyjiang/rho/commit/489a43f9cbfa67f60d9f6125e447e42ffb6d05b4))
* **tui:** allow limits during model turns ([#331](https://github.com/matthewyjiang/rho/issues/331)) ([2f1c63a](https://github.com/matthewyjiang/rho/commit/2f1c63ae31bc823845221c2d4e45bcb0cb87d494))
* **tui:** make pending discard accessible ([#335](https://github.com/matthewyjiang/rho/issues/335)) ([c3e18ff](https://github.com/matthewyjiang/rho/commit/c3e18ffe6098337c1fa9f099bd6d307acac13cee))
* **tui:** prevent markdown table parser panic on lone pipe lines ([#338](https://github.com/matthewyjiang/rho/issues/338)) ([e54fd50](https://github.com/matthewyjiang/rho/commit/e54fd507b53b36a2791aa9861d7fc7e60e6bdf09)), closes [#336](https://github.com/matthewyjiang/rho/issues/336)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.0.2 to 1.1.0

## [1.2.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.1.0...rho-coding-agent-v1.2.0) (2026-07-16)


### Features

* **goal:** pause goals blocked on user action ([#317](https://github.com/matthewyjiang/rho/issues/317)) ([31fc341](https://github.com/matthewyjiang/rho/commit/31fc341d02c6918a73cc517003ac9e1a160976d5))
* **subagents:** configurable subagent presets with herdr pane integration ([#323](https://github.com/matthewyjiang/rho/issues/323)) ([b19edd6](https://github.com/matthewyjiang/rho/commit/b19edd684486a6c25e26ba76fa76ce0927a7f95f))


### Bug Fixes

* **tui:** correct cumulative token tracking ([#318](https://github.com/matthewyjiang/rho/issues/318)) ([db092a3](https://github.com/matthewyjiang/rho/commit/db092a3960de3f863b6554215d528560388190f3))
* **tui:** toggle tool output on click ([#325](https://github.com/matthewyjiang/rho/issues/325)) ([564592c](https://github.com/matthewyjiang/rho/commit/564592c128c1dd49a91a374f983d0914997a3f95))

## [1.1.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.0.5...rho-coding-agent-v1.1.0) (2026-07-16)


### Features

* **tui:** add deterministic PTY test harness ([#303](https://github.com/matthewyjiang/rho/issues/303)) ([272615d](https://github.com/matthewyjiang/rho/commit/272615d21298b66bdb52455c5c6a807b4aebad57))
* **tui:** colorize markdown heading hierarchy ([#308](https://github.com/matthewyjiang/rho/issues/308)) ([312b06f](https://github.com/matthewyjiang/rho/commit/312b06f7bf62101cb7da71a23cb18bc77e469c22))


### Bug Fixes

* **tui:** prevent agent output from starving input ([#304](https://github.com/matthewyjiang/rho/issues/304)) ([7bdcc3d](https://github.com/matthewyjiang/rho/commit/7bdcc3d4fea82f9fddba5cc0bc07bf976b228575))
* **tui:** prioritize interaction rendering ([#311](https://github.com/matthewyjiang/rho/issues/311)) ([0f2c0a8](https://github.com/matthewyjiang/rho/commit/0f2c0a8f84804767cb31185953307f7fa4a31524))
* **tui:** run inline shell commands during turns ([#309](https://github.com/matthewyjiang/rho/issues/309)) ([890a848](https://github.com/matthewyjiang/rho/commit/890a84826e42e6f01377ddc17268fdadce5dba69))
* **tui:** simplify shell output presentation ([#307](https://github.com/matthewyjiang/rho/issues/307)) ([f98e49f](https://github.com/matthewyjiang/rho/commit/f98e49fc51a1efcdcf02c2b461b9c77e0ee977fd))

## [1.0.5](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.0.4...rho-coding-agent-v1.0.5) (2026-07-16)


### Bug Fixes

* **models:** restore GPT-5.6 context limits ([#300](https://github.com/matthewyjiang/rho/issues/300)) ([71f2785](https://github.com/matthewyjiang/rho/commit/71f278592a187aaaf143b02ccd56c4682b21c211))

## [1.0.4](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.0.3...rho-coding-agent-v1.0.4) (2026-07-15)


### Bug Fixes

* **provider:** handle callback stream bursts ([#288](https://github.com/matthewyjiang/rho/issues/288)) ([0c996cf](https://github.com/matthewyjiang/rho/commit/0c996cf97f116d689e4363d2f28091a87468a12d))
* **reasoning:** refresh incomplete model metadata ([#289](https://github.com/matthewyjiang/rho/issues/289)) ([c2115f3](https://github.com/matthewyjiang/rho/commit/c2115f3103847ddff15dcbcdeeb091998dee1451))

## [1.0.3](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.0.2...rho-coding-agent-v1.0.3) (2026-07-15)


### Bug Fixes

* **skills:** load discovered skills outside workspace ([#285](https://github.com/matthewyjiang/rho/issues/285)) ([386173b](https://github.com/matthewyjiang/rho/commit/386173bad15f6ceafbee129cc1f4308004f0f924))


### Performance Improvements

* reduce hot-path allocations and redundant I/O ([#280](https://github.com/matthewyjiang/rho/issues/280)) ([c18e582](https://github.com/matthewyjiang/rho/commit/c18e5823156254dccf59080864e775990c1b89cb))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.0.1 to 1.0.2

## [1.0.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.0.1...rho-coding-agent-v1.0.2) (2026-07-15)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.0.0 to 1.0.1

## [1.0.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v1.0.0...rho-coding-agent-v1.0.1) (2026-07-15)


### Bug Fixes

* **release:** separate application package ([#272](https://github.com/matthewyjiang/rho/issues/272)) ([55e8edf](https://github.com/matthewyjiang/rho/commit/55e8edfc950cddcd6ac94337d325a2408c8dac49))

## [1.0.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.29.1...rho-coding-agent-v1.0.0) (2026-07-15)


### Features

* **sdk:** add embeddable Rho runtime ([#262](https://github.com/matthewyjiang/rho/issues/262)) ([6fdac81](https://github.com/matthewyjiang/rho/commit/6fdac81b2a2d68331b72ecf768ad7631dada9d72))


### Bug Fixes

* **tui:** retry goal loop after incomplete runs ([#263](https://github.com/matthewyjiang/rho/issues/263)) ([35a6008](https://github.com/matthewyjiang/rho/commit/35a600899491129e290726e10067d08db1ce2498))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 0.1.0 to 1.0.0

## [0.29.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.29.0...rho-coding-agent-v0.29.1) (2026-07-15)


### Bug Fixes

* **anthropic:** use adaptive thinking effort ([#259](https://github.com/matthewyjiang/rho/issues/259)) ([39f875c](https://github.com/matthewyjiang/rho/commit/39f875c92e1196ba3255342656118fe1601371a0))
* **rtk:** record shell analytics ([#258](https://github.com/matthewyjiang/rho/issues/258)) ([66efc7b](https://github.com/matthewyjiang/rho/commit/66efc7b62c39eeb00803573bd5188fafdc3ec9f0))

## [0.29.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.28.1...rho-coding-agent-v0.29.0) (2026-07-14)


### Features

* **model:** preserve provider-native replay context ([#253](https://github.com/matthewyjiang/rho/issues/253)) ([3086c1a](https://github.com/matthewyjiang/rho/commit/3086c1a0f0dc11579efbf5b9573139afe239cb94))
* **tui:** add inline shell commands ([#254](https://github.com/matthewyjiang/rho/issues/254)) ([0cd61cd](https://github.com/matthewyjiang/rho/commit/0cd61cd14c9e4c011036221210125957fb57403d))

## [0.28.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.28.0...rho-coding-agent-v0.28.1) (2026-07-14)


### Bug Fixes

* **process:** render structured output ([#247](https://github.com/matthewyjiang/rho/issues/247)) ([2f422a3](https://github.com/matthewyjiang/rho/commit/2f422a3d81dbd9db4a57bacb8d2c34a352d4726a))
* **tui:** read Windows OSC palette responses ([#249](https://github.com/matthewyjiang/rho/issues/249)) ([51a3af3](https://github.com/matthewyjiang/rho/commit/51a3af37715e940724d66bc5588909410a402440))
* **tui:** reset jump button background ([#251](https://github.com/matthewyjiang/rho/issues/251)) ([45bc28c](https://github.com/matthewyjiang/rho/commit/45bc28cbd14c304cafbc77dee8e9fd54fff5df02))

## [0.28.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.27.1...rho-coding-agent-v0.28.0) (2026-07-14)


### Features

* **agent:** improve abort and steering lifecycle ([#238](https://github.com/matthewyjiang/rho/issues/238)) ([56b06cf](https://github.com/matthewyjiang/rho/commit/56b06cf3b9e09e03dfb8156cb5c5951385f21171))
* **export:** render LaTeX math in HTML transcripts ([#235](https://github.com/matthewyjiang/rho/issues/235)) ([c72d070](https://github.com/matthewyjiang/rho/commit/c72d07003356a63fa082c19d361c662759299a23))
* **tui:** make @ file search inline and directory-scoped ([#231](https://github.com/matthewyjiang/rho/issues/231)) ([f9bce1e](https://github.com/matthewyjiang/rho/commit/f9bce1e176ddf416837792f06b5e10f958edc28a))


### Bug Fixes

* **ci:** wait for descendant pid contents ([#236](https://github.com/matthewyjiang/rho/issues/236)) ([300a25e](https://github.com/matthewyjiang/rho/commit/300a25e031ab1d871993e991c5fef01bd9105c6e))
* **tui:** anchor spinner above composer ([#244](https://github.com/matthewyjiang/rho/issues/244)) ([432e64d](https://github.com/matthewyjiang/rho/commit/432e64d9b584a998c45f28368dadf68636e06fd2))
* **tui:** clarify goal command prompts ([#237](https://github.com/matthewyjiang/rho/issues/237)) ([e826729](https://github.com/matthewyjiang/rho/commit/e82672916a9887b13b6ef3b9d763af293b700f44))
* **tui:** preview streamed tool calls ([#243](https://github.com/matthewyjiang/rho/issues/243)) ([9d78c5d](https://github.com/matthewyjiang/rho/commit/9d78c5d74e05b0d0621d1f8c04ea024596ef3669))
* **tui:** show placeholder for hidden reasoning ([#242](https://github.com/matthewyjiang/rho/issues/242)) ([f1d1e8c](https://github.com/matthewyjiang/rho/commit/f1d1e8c534318dfa71c21e0aa62020bdb39f4dcf))

## [0.27.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.27.0...rho-coding-agent-v0.27.1) (2026-07-13)


### Bug Fixes

* **reasoning:** rehydrate incomplete model effort metadata ([#224](https://github.com/matthewyjiang/rho/issues/224)) ([85405ad](https://github.com/matthewyjiang/rho/commit/85405ad69411485121ce1a7c32ff72cb2ca4d28b))
* **tui:** keep mouse wheel working under wezterm on windows ([#228](https://github.com/matthewyjiang/rho/issues/228)) ([0c947c6](https://github.com/matthewyjiang/rho/commit/0c947c66c3f70248f078004e7d3ae28071550a49))
* **tui:** keep shift+tab on windows under conpty ([#226](https://github.com/matthewyjiang/rho/issues/226)) ([69fc13c](https://github.com/matthewyjiang/rho/commit/69fc13ccb25d1fab2f1da5eb410bd64b97473db5))
* **update:** detect Scoop installs on Windows ([#225](https://github.com/matthewyjiang/rho/issues/225)) ([f9c50ec](https://github.com/matthewyjiang/rho/commit/f9c50ec3b52c003fa38e5695e6ee6435e6658bbf))

## [0.27.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.26.0...rho-coding-agent-v0.27.0) (2026-07-13)


### Features

* **reasoning:** use model-supported effort levels ([#221](https://github.com/matthewyjiang/rho/issues/221)) ([54a5190](https://github.com/matthewyjiang/rho/commit/54a51908ae0b9664db1991c30857a35aa0f2d584))
* **xai:** add OAuth provider support ([#220](https://github.com/matthewyjiang/rho/issues/220)) ([b053c99](https://github.com/matthewyjiang/rho/commit/b053c9993e0df57fd27c07c191a4fbf594acc51b))

## [0.26.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.25.0...rho-coding-agent-v0.26.0) (2026-07-13)


### Features

* **diagnostics:** add rho runtime introspection ([#215](https://github.com/matthewyjiang/rho/issues/215)) ([6ff60f6](https://github.com/matthewyjiang/rho/commit/6ff60f62d452debf8d4adc1e76128142a84bcaa9))
* **tools:** add atomic multi-file edits ([#217](https://github.com/matthewyjiang/rho/issues/217)) ([f0318e0](https://github.com/matthewyjiang/rho/commit/f0318e03cc597c5cb535c41945a3440d9aed17e3))
* **tui:** add /export HTML session transcript command ([#216](https://github.com/matthewyjiang/rho/issues/216)) ([c0abace](https://github.com/matthewyjiang/rho/commit/c0abacec3b775ada2111e4a13dc664e72f539041))


### Bug Fixes

* **models:** reduce GPT-5.6 context limits ([#213](https://github.com/matthewyjiang/rho/issues/213)) ([2f52408](https://github.com/matthewyjiang/rho/commit/2f524085743293dd540ac9b530c08175d4bdc9a2))

## [0.25.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.24.1...rho-coding-agent-v0.25.0) (2026-07-13)


### ⚠ BREAKING CHANGES

* **tools:** start_process, poll_process, write_process, stop_process, and list_processes are replaced by the process tool with start, poll, and stop actions. Process stdin writing and listing are no longer supported.

### Features

* **prompt:** strengthen coding agent guidance ([#209](https://github.com/matthewyjiang/rho/issues/209)) ([3056bcc](https://github.com/matthewyjiang/rho/commit/3056bccddbd416943b636ea40dfad3eee7723508))
* **tui:** add OAuth usage limits command ([#210](https://github.com/matthewyjiang/rho/issues/210)) ([cef1f80](https://github.com/matthewyjiang/rho/commit/cef1f808a913a82161091aef5593ec6893d5fbe9))


### Bug Fixes

* address performance, rendering, and correctness audit ([#207](https://github.com/matthewyjiang/rho/issues/207)) ([28d4ac5](https://github.com/matthewyjiang/rho/commit/28d4ac56840ea67479e7ac292494f74495287837))


### Code Refactoring

* **tools:** collapse background process controls ([#205](https://github.com/matthewyjiang/rho/issues/205)) ([bfa31ae](https://github.com/matthewyjiang/rho/commit/bfa31ae744ff9d2a3dd45e083f912ebd9d8b5913))

## [0.24.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.24.0...rho-coding-agent-v0.24.1) (2026-07-12)


### Bug Fixes

* **ci:** treat zombie descendants as terminated ([#202](https://github.com/matthewyjiang/rho/issues/202)) ([9dae958](https://github.com/matthewyjiang/rho/commit/9dae95869556b3bfe8d5411a800bd22c46868708))

## [0.24.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.23.0...rho-coding-agent-v0.24.0) (2026-07-12)


### Features

* **tools:** add managed background processes ([#199](https://github.com/matthewyjiang/rho/issues/199)) ([9c6e75f](https://github.com/matthewyjiang/rho/commit/9c6e75f792e20041cfd2221e4fc170f04bcebbd3))
* **tui:** add colors for web and questionnaire tools ([#191](https://github.com/matthewyjiang/rho/issues/191)) ([01d7003](https://github.com/matthewyjiang/rho/commit/01d70037879219451a029778221f899d36e51af6))
* **tui:** add custom prompt templates ([#196](https://github.com/matthewyjiang/rho/issues/196)) ([f4b0ec1](https://github.com/matthewyjiang/rho/commit/f4b0ec139d2f1e062d7979aa682f0ed93e0e6363))
* **tui:** use fuzzy model search ([#197](https://github.com/matthewyjiang/rho/issues/197)) ([0e82857](https://github.com/matthewyjiang/rho/commit/0e82857a691827c89d5ef3c2aa413bcc20f3a1c0))


### Bug Fixes

* **openai:** stream Codex websocket events immediately ([#200](https://github.com/matthewyjiang/rho/issues/200)) ([1f28a48](https://github.com/matthewyjiang/rho/commit/1f28a48386b52f73de34226bb16f3a734c009169))
* **tui:** support ansi dimming on windows ([#192](https://github.com/matthewyjiang/rho/issues/192)) ([5a4d793](https://github.com/matthewyjiang/rho/commit/5a4d7933eb7a24013cdf37b5985150d5b80225c7))


### Performance Improvements

* optimize streaming, rendering, and session hot paths ([#198](https://github.com/matthewyjiang/rho/issues/198)) ([d043d1f](https://github.com/matthewyjiang/rho/commit/d043d1f3b580f3f62b82cc023ade095140c032ca))
* **tui:** keep input responsive during agent output ([#195](https://github.com/matthewyjiang/rho/issues/195)) ([4a8f63d](https://github.com/matthewyjiang/rho/commit/4a8f63dad80ef10c705a5d30c6dc3a0e34606772))

## [0.23.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.22.1...rho-coding-agent-v0.23.0) (2026-07-11)


### Features

* **config:** organize settings and add keybindings ([#186](https://github.com/matthewyjiang/rho/issues/186)) ([394aa35](https://github.com/matthewyjiang/rho/commit/394aa35d62af2b1f3e6558a39dedc188c063332f))
* **tui:** add diff and doctor commands ([#187](https://github.com/matthewyjiang/rho/issues/187)) ([4b59d6f](https://github.com/matthewyjiang/rho/commit/4b59d6f04e9ac92a72d27770c20926c5dbe36344))
* **tui:** add goal command ([#189](https://github.com/matthewyjiang/rho/issues/189)) ([e134003](https://github.com/matthewyjiang/rho/commit/e134003d8e5e70a32025f4437e3389bb54c7bfa3))


### Bug Fixes

* **agent:** bound invalid response retries ([#179](https://github.com/matthewyjiang/rho/issues/179)) ([168c6e3](https://github.com/matthewyjiang/rho/commit/168c6e3337879956d358fe515eb6d3c6b8de9b8a))
* **openai:** accept compact SSE data fields ([#180](https://github.com/matthewyjiang/rho/issues/180)) ([99e5da4](https://github.com/matthewyjiang/rho/commit/99e5da4e4dde45f22c6a17d982c340696d3889fa))
* **tools:** terminate bash process groups on timeout ([#181](https://github.com/matthewyjiang/rho/issues/181)) ([eb4863c](https://github.com/matthewyjiang/rho/commit/eb4863c1b5fa80560d1ee840c516b23e59bdcd8b))
* **tui:** interrupt active tool calls on esc ([#184](https://github.com/matthewyjiang/rho/issues/184)) ([8c08843](https://github.com/matthewyjiang/rho/commit/8c088436a7100479aaf75612c4354bfddafcdb9f))
* **tui:** keep running after cancelling questionnaire ([#182](https://github.com/matthewyjiang/rho/issues/182)) ([891aab6](https://github.com/matthewyjiang/rho/commit/891aab65a228cca2c511b8715b6bdfaaae5b8dab))

## [0.22.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.22.0...rho-coding-agent-v0.22.1) (2026-07-10)


### Bug Fixes

* **codex:** validate websocket continuation state ([#177](https://github.com/matthewyjiang/rho/issues/177)) ([41479d2](https://github.com/matthewyjiang/rho/commit/41479d26c04cbf06336fbe6b9d333acd397809c3))
* **tui:** capture mouse wheel events on windows ([#174](https://github.com/matthewyjiang/rho/issues/174)) ([cba1488](https://github.com/matthewyjiang/rho/commit/cba14886e1e46b374779bc271c53e9640a489c91))

## [0.22.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.21.1...rho-coding-agent-v0.22.0) (2026-07-10)


### Features

* **tools:** display colored file diffs ([#169](https://github.com/matthewyjiang/rho/issues/169)) ([b73e0fb](https://github.com/matthewyjiang/rho/commit/b73e0fb83fd88eae159d82a37a62281eba28afb0))


### Bug Fixes

* **model:** recover from stalled provider streams ([#171](https://github.com/matthewyjiang/rho/issues/171)) ([7232799](https://github.com/matthewyjiang/rho/commit/7232799e83efd97d9cd25ba18087a71ffed3f899))
* **tui:** restore transcript copy support ([#168](https://github.com/matthewyjiang/rho/issues/168)) ([8b9da7e](https://github.com/matthewyjiang/rho/commit/8b9da7ea37739934080a621e068d99ebf2ea8a09))

## [0.21.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.21.0...rho-coding-agent-v0.21.1) (2026-07-10)


### Bug Fixes

* **codex:** prevent Sol tool call loops ([#165](https://github.com/matthewyjiang/rho/issues/165)) ([2a16eca](https://github.com/matthewyjiang/rho/commit/2a16ecaba342e952b0ce09ab830481232c402ba0))

## [0.21.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.20.0...rho-coding-agent-v0.21.0) (2026-07-09)


### Features

* **tui:** render markdown tables ([#160](https://github.com/matthewyjiang/rho/issues/160)) ([2c2b83c](https://github.com/matthewyjiang/rho/commit/2c2b83cb4c3961ac07441fa0505170915721f6b2))


### Bug Fixes

* **codex:** align responses lite transport ([#162](https://github.com/matthewyjiang/rho/issues/162)) ([0791033](https://github.com/matthewyjiang/rho/commit/0791033d7c4796a328fa79254e2fbd3d6b8d3993))
* **tui:** fill tool rows containing control characters ([#159](https://github.com/matthewyjiang/rho/issues/159)) ([cd37440](https://github.com/matthewyjiang/rho/commit/cd37440837a22cec57ad6c2d2795eb052c4092e6))

## [0.20.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.19.0...rho-coding-agent-v0.20.0) (2026-07-09)


### Features

* **models:** add pinned model picker favorites ([#148](https://github.com/matthewyjiang/rho/issues/148)) ([ee8b5bc](https://github.com/matthewyjiang/rho/commit/ee8b5bc79abaaad8d443aea25c765ec694af8942))
* **tui:** add manual compact command ([#151](https://github.com/matthewyjiang/rho/issues/151)) ([1c39dec](https://github.com/matthewyjiang/rho/commit/1c39decd88aeee721c7cb855d9b1d12b2adb0710))
* **tui:** autocomplete file paths with @ ([#155](https://github.com/matthewyjiang/rho/issues/155)) ([beaaa9f](https://github.com/matthewyjiang/rho/commit/beaaa9fe84865a15fe9ffccc97362264b261ecb4))


### Bug Fixes

* **reasoning:** map codex effort by model ([#150](https://github.com/matthewyjiang/rho/issues/150)) ([e4a95e1](https://github.com/matthewyjiang/rho/commit/e4a95e14bc7faef48f431260680f36f06a0a7112))
* **tui:** defer model changes until agent run ends ([#152](https://github.com/matthewyjiang/rho/issues/152)) ([78dfd33](https://github.com/matthewyjiang/rho/commit/78dfd33344d0f4c39767ecd601d642a1b5a64b8a))
* **tui:** filter logout provider picker ([#153](https://github.com/matthewyjiang/rho/issues/153)) ([8a9e449](https://github.com/matthewyjiang/rho/commit/8a9e449df6e18c5833da6c19347eb74621799cdc))
* **tui:** hide inactive history scrollbar ([#147](https://github.com/matthewyjiang/rho/issues/147)) ([8db9843](https://github.com/matthewyjiang/rho/commit/8db9843ddaac2dc3f246452d1e5c1a91886c7e06))
* **tui:** restore multiline paste handling on Windows ([#157](https://github.com/matthewyjiang/rho/issues/157)) ([6d63adf](https://github.com/matthewyjiang/rho/commit/6d63adf4b0ea56d643fcb56ccf7034de9c3d1489))

## [0.19.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.18.1...rho-coding-agent-v0.19.0) (2026-07-09)


### Features

* **tui:** add app-owned transcript scrolling ([#144](https://github.com/matthewyjiang/rho/issues/144)) ([a62dbb5](https://github.com/matthewyjiang/rho/commit/a62dbb5bb9132157f45b642ec9c37d2bd80b938b))

## [0.18.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.18.0...rho-coding-agent-v0.18.1) (2026-07-09)


### Bug Fixes

* **model:** use copilot api models without fallback ([#140](https://github.com/matthewyjiang/rho/issues/140)) ([c5756d8](https://github.com/matthewyjiang/rho/commit/c5756d80750177d62ffd524cf376c8d17f7b13e5))
* **tui:** collapse pasted key bursts ([#139](https://github.com/matthewyjiang/rho/issues/139)) ([805fb4d](https://github.com/matthewyjiang/rho/commit/805fb4db38db5039557617ee9026f0ef9d1987f6))
* **tui:** place spinner above input box ([#141](https://github.com/matthewyjiang/rho/issues/141)) ([edb3093](https://github.com/matthewyjiang/rho/commit/edb3093780e6100e05e70bf159f2c2ce8f4b9b41))

## [0.18.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.17.1...rho-coding-agent-v0.18.0) (2026-07-09)


### Features

* **agent:** add auto compaction ([#131](https://github.com/matthewyjiang/rho/issues/131)) ([e4dbc74](https://github.com/matthewyjiang/rho/commit/e4dbc74f5c32a6d1c57a856e86979ba483f6f45c))

## [0.17.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.17.0...rho-coding-agent-v0.17.1) (2026-07-08)


### Bug Fixes

* **ci:** repair Arch package workflow ([#133](https://github.com/matthewyjiang/rho/issues/133)) ([8d351e7](https://github.com/matthewyjiang/rho/commit/8d351e7549217a4ebd94d7077f61882583a31d5f))

## [0.17.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.16.1...rho-coding-agent-v0.17.0) (2026-07-08)


### Features

* **auth:** add Codex device login ([#128](https://github.com/matthewyjiang/rho/issues/128)) ([135efef](https://github.com/matthewyjiang/rho/commit/135efeff41672a52b69c4819c76d4be2b8931f6c))
* **questionnaire:** add user question tool ([#127](https://github.com/matthewyjiang/rho/issues/127)) ([3ccef80](https://github.com/matthewyjiang/rho/commit/3ccef805ea852e54a5df00ad559189fd84ca8af7))

## [0.16.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.16.0...rho-coding-agent-v0.16.1) (2026-07-07)


### Bug Fixes

* **tui:** preview partial streaming lines ([#122](https://github.com/matthewyjiang/rho/issues/122)) ([2aa570f](https://github.com/matthewyjiang/rho/commit/2aa570f0c165f16b6b0fa86563a9ae03bbf3c87c))
* **tui:** remove status info line ([#123](https://github.com/matthewyjiang/rho/issues/123)) ([926666f](https://github.com/matthewyjiang/rho/commit/926666fb2ad9e4d5c725308192707c66b8915e9e))

## [0.16.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.15.3...rho-coding-agent-v0.16.0) (2026-07-07)


### Features

* **herdr:** report rho agent state ([#117](https://github.com/matthewyjiang/rho/issues/117)) ([fa4c18b](https://github.com/matthewyjiang/rho/commit/fa4c18bfa27fc19aa8b6094d19d8e0f47974c790))
* **tui:** style input by reasoning level ([#119](https://github.com/matthewyjiang/rho/issues/119)) ([8b9a71c](https://github.com/matthewyjiang/rho/commit/8b9a71c26efaf5a45a171be8204731b4149ef6ce))


### Bug Fixes

* **tui:** stabilize live rendering ([#120](https://github.com/matthewyjiang/rho/issues/120)) ([9ad7a9c](https://github.com/matthewyjiang/rho/commit/9ad7a9c42d3abb3cccc0ce881c212b2f1d0ab018))

## [0.15.3](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.15.2...rho-coding-agent-v0.15.3) (2026-07-04)


### Bug Fixes

* **commands:** treat multiline slash input as prompt ([#115](https://github.com/matthewyjiang/rho/issues/115)) ([1f0a0f5](https://github.com/matthewyjiang/rho/commit/1f0a0f59e0a927545c389db9606cfd371dc52906))
* **tools:** improve shell output robustness and edit/read validation ([#112](https://github.com/matthewyjiang/rho/issues/112)) ([6a0f1b1](https://github.com/matthewyjiang/rho/commit/6a0f1b16d4279eb268f93460a2b032ab5bb95c7b))
* **tui:** avoid rerendering history on viewport height changes ([#114](https://github.com/matthewyjiang/rho/issues/114)) ([2436480](https://github.com/matthewyjiang/rho/commit/2436480536f6ec8a0ef1b60e78e4a5614a2ab632))

## [0.15.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.15.1...rho-coding-agent-v0.15.2) (2026-07-01)


### Bug Fixes

* **edit-file:** normalize line endings for replacements ([#109](https://github.com/matthewyjiang/rho/issues/109)) ([c1b7b44](https://github.com/matthewyjiang/rho/commit/c1b7b4484fc2ff146c32593a470e1ac4acf69dfc))

## [0.15.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.15.0...rho-coding-agent-v0.15.1) (2026-07-01)


### Bug Fixes

* **update:** avoid automatic Windows updates ([#106](https://github.com/matthewyjiang/rho/issues/106)) ([738b4ed](https://github.com/matthewyjiang/rho/commit/738b4ed61241401bb08b9a6efec289f06b30e005))

## [0.15.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.14.1...rho-coding-agent-v0.15.0) (2026-06-30)


### Features

* **tools:** add built-in rtk command rewriting ([#103](https://github.com/matthewyjiang/rho/issues/103)) ([57e890b](https://github.com/matthewyjiang/rho/commit/57e890b05b43d7f0ade80c8050d4c1751fdbcdea))

## [0.14.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.14.0...rho-coding-agent-v0.14.1) (2026-06-30)


### Bug Fixes

* **windows:** avoid suspicious powershell install patterns ([#96](https://github.com/matthewyjiang/rho/issues/96)) ([6dd8009](https://github.com/matthewyjiang/rho/commit/6dd80098dac360c0532d13e255ae5fa28a316049))

## [0.14.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.13.1...rho-coding-agent-v0.14.0) (2026-06-30)


### Features

* **auth:** use device code login for GitHub Copilot ([#91](https://github.com/matthewyjiang/rho/issues/91)) ([7abae3c](https://github.com/matthewyjiang/rho/commit/7abae3cacdd87a22d95b6bba64a0b98670fbd2f4))


### Bug Fixes

* **tui:** collapse pasted input markers ([#94](https://github.com/matthewyjiang/rho/issues/94)) ([3cd0758](https://github.com/matthewyjiang/rho/commit/3cd07588ca1112ff6e30229bdb6921e35116d11f))
* **update:** defer windows executable replacement ([#92](https://github.com/matthewyjiang/rho/issues/92)) ([ffd839d](https://github.com/matthewyjiang/rho/commit/ffd839d3480fd5a94951065a0cc34444c2087e2f))

## [0.13.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.13.0...rho-coding-agent-v0.13.1) (2026-06-29)


### Bug Fixes

* **anthropic:** sanitize tool schemas ([#89](https://github.com/matthewyjiang/rho/issues/89)) ([7016a93](https://github.com/matthewyjiang/rho/commit/7016a93e38321c1eb8e63ab4a0a95157b103a64a))

## [0.13.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.12.1...rho-coding-agent-v0.13.0) (2026-06-29)


### Features

* **images:** support message image attachments ([#88](https://github.com/matthewyjiang/rho/issues/88)) ([5bef034](https://github.com/matthewyjiang/rho/commit/5bef034d3172320d784d5dd4546a9cf1f516b8a3))
* **provider:** add github copilot provider ([#81](https://github.com/matthewyjiang/rho/issues/81)) ([fc52828](https://github.com/matthewyjiang/rho/commit/fc528286ea685b3f88b206131eea49c6e9091683))
* **update:** add update checks and command ([#87](https://github.com/matthewyjiang/rho/issues/87)) ([b889907](https://github.com/matthewyjiang/rho/commit/b8899076ae78b7ae9f0fce9f3d17300299fd82a5))


### Bug Fixes

* **tui:** show tool calls before results ([#83](https://github.com/matthewyjiang/rho/issues/83)) ([b95727d](https://github.com/matthewyjiang/rho/commit/b95727df7d0b25567194e73de675d2874a0c5f24))

## [0.12.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.12.0...rho-coding-agent-v0.12.1) (2026-06-27)


### Bug Fixes

* **tui:** improve block foreground contrast ([#78](https://github.com/matthewyjiang/rho/issues/78)) ([12ae234](https://github.com/matthewyjiang/rho/commit/12ae2348111d161300be89973cf9d519b1970483))
* **web:** use hosted codex search ([#79](https://github.com/matthewyjiang/rho/issues/79)) ([41a4227](https://github.com/matthewyjiang/rho/commit/41a422736b89b21b8c3df00dded3448683d89c9f))

## [0.12.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.11.0...rho-coding-agent-v0.12.0) (2026-06-26)


### Features

* **tui:** add new session slash command ([#73](https://github.com/matthewyjiang/rho/issues/73)) ([f8a4bde](https://github.com/matthewyjiang/rho/commit/f8a4bde7ff8673a7460bc6c34cdb985bf95877a1))
* **tui:** add prompt history and queued edits ([#68](https://github.com/matthewyjiang/rho/issues/68)) ([8bf14bd](https://github.com/matthewyjiang/rho/commit/8bf14bdf6c90d48279b0be8d07609390fe2745ee))
* **tui:** add reasoning output toggle ([#72](https://github.com/matthewyjiang/rho/issues/72)) ([5fd989c](https://github.com/matthewyjiang/rho/commit/5fd989c28aeb86e928dd6f7929f8f81d17770b2f))
* **web:** add zero-config web access tools ([#77](https://github.com/matthewyjiang/rho/issues/77)) ([e3a0052](https://github.com/matthewyjiang/rho/commit/e3a00527af165a1fce3dad53a81716847c1f420a))

## [0.11.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.10.0...rho-coding-agent-v0.11.0) (2026-06-24)


### Features

* **packaging:** add Arch Linux package and mjiang-extras publishing ([#61](https://github.com/matthewyjiang/rho/issues/61)) ([c419184](https://github.com/matthewyjiang/rho/commit/c419184933805ba8d2da814a7fc26124678f25ba))


### Bug Fixes

* **packaging:** disable LTO to fix linking with Arch's lld ([#65](https://github.com/matthewyjiang/rho/issues/65)) ([11fbb4d](https://github.com/matthewyjiang/rho/commit/11fbb4d11fddf82e1ad5035a63d4e7d4b6731c59))
* **packaging:** set SQLITE3_LIB_DIR to fix system SQLite linking ([#63](https://github.com/matthewyjiang/rho/issues/63)) ([1803d5b](https://github.com/matthewyjiang/rho/commit/1803d5b5fae93d62e651c1ea47326e488b504971))
* **packaging:** use bundled SQLite instead of fighting system detection ([#64](https://github.com/matthewyjiang/rho/issues/64)) ([defb2f7](https://github.com/matthewyjiang/rho/commit/defb2f7e96d87746690f98b4317c47cd0ccc3456))
* **packaging:** use system SQLite and disable LTO ([#66](https://github.com/matthewyjiang/rho/issues/66)) ([d91b7d9](https://github.com/matthewyjiang/rho/commit/d91b7d9bde5144c97f1c8739db9a2db140430966))

## [0.10.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.9.3...rho-coding-agent-v0.10.0) (2026-06-24)


### Features

* **model:** add anthropic provider and registry ([#60](https://github.com/matthewyjiang/rho/issues/60)) ([037468c](https://github.com/matthewyjiang/rho/commit/037468c466baa2d305f37fdc001376a38838a0a7))


### Bug Fixes

* **auth:** chunk large windows credentials ([#57](https://github.com/matthewyjiang/rho/issues/57)) ([8515a41](https://github.com/matthewyjiang/rho/commit/8515a4177014dd6bcc08be9ffe64331649e27f20))

## [0.9.3](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.9.2...rho-coding-agent-v0.9.3) (2026-06-24)


### Bug Fixes

* **auth:** use localhost for Codex OAuth redirect ([#55](https://github.com/matthewyjiang/rho/issues/55)) ([1484983](https://github.com/matthewyjiang/rho/commit/14849830444bca9e9c6ebe7c0c638d77f3c830df))
* **windows:** silence terminal theme warnings ([#54](https://github.com/matthewyjiang/rho/issues/54)) ([88c3e33](https://github.com/matthewyjiang/rho/commit/88c3e33bcac46db8668fcb7964c3b006851d6af5))

## [0.9.2](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.9.1...rho-coding-agent-v0.9.2) (2026-06-24)


### Bug Fixes

* **tui:** tolerate unsupported keyboard enhancements ([#51](https://github.com/matthewyjiang/rho/issues/51)) ([176283c](https://github.com/matthewyjiang/rho/commit/176283c98ac5a0b7123ae3793e0dd15d5af000a4))

## [0.9.1](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.9.0...rho-coding-agent-v0.9.1) (2026-06-24)


### Bug Fixes

* **windows:** improve installer and shell support ([#47](https://github.com/matthewyjiang/rho/issues/47)) ([4d6da5b](https://github.com/matthewyjiang/rho/commit/4d6da5b7912a9a76a0b1cceee5ce491f45d7ed8f))

## [0.9.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.8.0...rho-coding-agent-v0.9.0) (2026-06-23)


### Features

* **install:** add prebuilt binary installers ([#45](https://github.com/matthewyjiang/rho/issues/45)) ([247538d](https://github.com/matthewyjiang/rho/commit/247538dac72b854c49adcac222f63ad33f877b3a))

## [0.8.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.7.0...rho-coding-agent-v0.8.0) (2026-06-23)


### Features

* **cli:** add context override flags ([#42](https://github.com/matthewyjiang/rho/issues/42)) ([53aa63e](https://github.com/matthewyjiang/rho/commit/53aa63e8c69607781eb9e3a722abc6b3005a0306))

## [0.7.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.6.0...rho-coding-agent-v0.7.0) (2026-06-23)


### Features

* **agent:** optimize token context handling ([#28](https://github.com/matthewyjiang/rho/issues/28)) ([e3957c2](https://github.com/matthewyjiang/rho/commit/e3957c2e1d2b40b15692423e0c0f001b1d9c870b))
* **tui:** add ansi theme and markdown rendering ([#32](https://github.com/matthewyjiang/rho/issues/32)) ([ad9b3f6](https://github.com/matthewyjiang/rho/commit/ad9b3f6430723e080f84b8c575121af3796d60d3))
* **tui:** add ansi themed markdown rendering ([#35](https://github.com/matthewyjiang/rho/issues/35)) ([976accd](https://github.com/matthewyjiang/rho/commit/976accd5bb39692bea7dbf00d2961d9b24dd2ad3))
* **tui:** add loading spinner for generation ([#37](https://github.com/matthewyjiang/rho/issues/37)) ([05d9600](https://github.com/matthewyjiang/rho/commit/05d9600d3dd005c45482a1f916923905ef6227a9))
* **tui:** keep composer interactive during turns ([#39](https://github.com/matthewyjiang/rho/issues/39)) ([2fcde2c](https://github.com/matthewyjiang/rho/commit/2fcde2cebb745f02b2671eef347246312aa7c68f))
* **tui:** truncate tool output lines ([#26](https://github.com/matthewyjiang/rho/issues/26)) ([5479372](https://github.com/matthewyjiang/rho/commit/54793729d1bc65bb3ab411a3631d4e0de8f5b9b5))


### Bug Fixes

* **tui:** anchor inline status line ([#36](https://github.com/matthewyjiang/rho/issues/36)) ([2716fd2](https://github.com/matthewyjiang/rho/commit/2716fd2604dc739239819f11437e9c7f9b57d33a))
* **tui:** clamp inline picker text ([#40](https://github.com/matthewyjiang/rho/issues/40)) ([89cc49a](https://github.com/matthewyjiang/rho/commit/89cc49ab5754e077d9eed5db4a2147cee911424a))
* **tui:** make model streaming append-only ([#30](https://github.com/matthewyjiang/rho/issues/30)) ([3878383](https://github.com/matthewyjiang/rho/commit/3878383ddbd6a084c61d945cdcae95bebd3660b4))
* **tui:** use latest response for cache hit rate ([#38](https://github.com/matthewyjiang/rho/issues/38)) ([136f054](https://github.com/matthewyjiang/rho/commit/136f054e190402679bed5993d641e261c7daa205))

## [0.6.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.5.0...rho-coding-agent-v0.6.0) (2026-06-22)


### Features

* **session:** add interactive resume picker ([#24](https://github.com/matthewyjiang/rho/issues/24)) ([19c5809](https://github.com/matthewyjiang/rho/commit/19c5809da34bb75f051d32062d3274d99ad54858))
* **tui:** add config picker and statusline ([#23](https://github.com/matthewyjiang/rho/issues/23)) ([2a179f7](https://github.com/matthewyjiang/rho/commit/2a179f7794967be6532772d7bbd40a2660fd2f31))


### Bug Fixes

* **tui:** reflow transcript on resize ([#21](https://github.com/matthewyjiang/rho/issues/21)) ([910518c](https://github.com/matthewyjiang/rho/commit/910518c9b4f0198ece896ed9b0d32d3f1f721d95))

## [0.5.0](https://github.com/matthewyjiang/rho/compare/rho-coding-agent-v0.4.0...rho-coding-agent-v0.5.0) (2026-06-22)


### Features

* **auth:** add interactive login credentials ([#18](https://github.com/matthewyjiang/rho/issues/18)) ([fd6a38e](https://github.com/matthewyjiang/rho/commit/fd6a38ee300dda9df429b059e22df56e24490b59))
* **skills:** add skill discovery and slash loading ([#19](https://github.com/matthewyjiang/rho/issues/19)) ([a0090de](https://github.com/matthewyjiang/rho/commit/a0090de12c408522bdcf0beb7f5c7c1f726741e0))


### Bug Fixes

* **release:** use stable release please component ([#16](https://github.com/matthewyjiang/rho/issues/16)) ([74df53a](https://github.com/matthewyjiang/rho/commit/74df53a59f0705db48028152b748c4cc1b9af97b))

## [0.4.0](https://github.com/matthewyjiang/rho/compare/rho-agent-v0.3.0...rho-coding-agent-v0.4.0) (2026-06-22)


### Features

* **model:** add static model catalog ([#10](https://github.com/matthewyjiang/rho/issues/10)) ([2f568a7](https://github.com/matthewyjiang/rho/commit/2f568a7518e1fecfa50693e2d1adb4d9386f8e89))

## [0.3.0](https://github.com/matthewyjiang/rho/compare/rho-agent-v0.2.0...rho-agent-v0.3.0) (2026-06-21)


### Features

* **session:** persist interactive sessions ([#8](https://github.com/matthewyjiang/rho/issues/8)) ([6b9393d](https://github.com/matthewyjiang/rho/commit/6b9393d272ef8b99b9aaaf1167d283888452ff48))


### Bug Fixes

* **docs:** set VitePress pages base ([#6](https://github.com/matthewyjiang/rho/issues/6)) ([af18224](https://github.com/matthewyjiang/rho/commit/af182243a8edae15ffde25a6e1fe1e1d4d84ef8c))

## [0.2.0](https://github.com/matthewyjiang/rho/compare/rho-agent-v0.1.0...rho-agent-v0.2.0) (2026-06-21)


### Features

* **tui:** add interactive ratatui shell ([#3](https://github.com/matthewyjiang/rho/issues/3)) ([85062d7](https://github.com/matthewyjiang/rho/commit/85062d72b5c59406431357d013250698efbec8f1))

## 0.1.0 (2026-06-21)


### Features

* **auth:** support codex oauth auth ([100233d](https://github.com/matthewyjiang/rho/commit/100233d2f5315254fc6ca195df8a5c031f9ca04d))
* **config:** persist provider and use current working directory ([179526b](https://github.com/matthewyjiang/rho/commit/179526b20f63fc56633aceb6f972acfd96a5a1b7))
* **history:** add persistent message history ([b28c1e9](https://github.com/matthewyjiang/rho/commit/b28c1e9e736161d934cd8cca7ff3f3111c5f242a))
* **model:** support openai tool call blocks ([9223c25](https://github.com/matthewyjiang/rho/commit/9223c252b9653b35035d2978120492724c8f730b))


### Bug Fixes

* **prompt:** harden prompt tool call parsing ([006258f](https://github.com/matthewyjiang/rho/commit/006258f61a51cf8aa6899eae5a1e62ff62e1b6ca))
* **prompt:** parse first fenced json object ([bdf6236](https://github.com/matthewyjiang/rho/commit/bdf62366257037356040f9bfd8ebc5e9e059b138))
