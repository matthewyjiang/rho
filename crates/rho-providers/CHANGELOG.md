# Changelog

## [2.0.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v1.2.0...rho-providers-v2.0.0) (2026-08-13)


### ⚠ BREAKING CHANGES

* **xai:** ModelEvent::WebSearch and RunEvent::WebSearch now carry a name field so hosts can distinguish web_search from x_search.

### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **agents:** add Claude Code subagent runtime ([#541](https://github.com/matthewyjiang/rho/issues/541)) ([c1385ec](https://github.com/matthewyjiang/rho/commit/c1385ecae9b2eb967ae73ecc09c20cc80bc63479))
* **agents:** allow pinning auth profiles on rho agents ([#781](https://github.com/matthewyjiang/rho/issues/781)) ([3e1f691](https://github.com/matthewyjiang/rho/commit/3e1f691e693dd93bf888cb5c3eb3093a7169525a))
* **agents:** move background-run contract into tool descriptions ([#405](https://github.com/matthewyjiang/rho/issues/405)) ([b75d0fa](https://github.com/matthewyjiang/rho/commit/b75d0fac659cd85a5469ce962e2bd026c673e288))
* **auth:** add active auth mode switcher ([#609](https://github.com/matthewyjiang/rho/issues/609)) ([a2b0f68](https://github.com/matthewyjiang/rho/commit/a2b0f68f71033ca6f8594a35368a79b2388916ca))
* **auth:** add configurable credential storage ([#478](https://github.com/matthewyjiang/rho/issues/478)) ([e778eda](https://github.com/matthewyjiang/rho/commit/e778edab71ec7e3c2f21137760f53bd0b8089469))
* **auth:** add OpenRouter OAuth login ([#472](https://github.com/matthewyjiang/rho/issues/472)) ([42af8e7](https://github.com/matthewyjiang/rho/commit/42af8e7a95bc1d16245f89dd1ebe74e6c4f56b7b))
* **cli:** add structured run output ([#467](https://github.com/matthewyjiang/rho/issues/467)) ([c4088bb](https://github.com/matthewyjiang/rho/commit/c4088bb03ef0e7e1b69de5e671773399755fe07b))
* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **permission:** rename Auto to Bypass and add classifier Auto ([#870](https://github.com/matthewyjiang/rho/issues/870)) ([3192daa](https://github.com/matthewyjiang/rho/commit/3192daa713f7202f44727ec4acb83d0c646d1286))
* **permission:** screen Auto requests with a two-stage classifier ([#893](https://github.com/matthewyjiang/rho/issues/893)) ([4149f11](https://github.com/matthewyjiang/rho/commit/4149f1157aa2c8ee10561a21a919a0e530b8f3cc))
* **prompt:** tell the agent which model runs it, its subagents, and the advisor ([#860](https://github.com/matthewyjiang/rho/issues/860)) ([d18c377](https://github.com/matthewyjiang/rho/commit/d18c3774a20657cc1214e2936251cc993b69dd14))
* **providers:** add Meta Model API and collapse provider registration ([#755](https://github.com/matthewyjiang/rho/issues/755)) ([b41ef92](https://github.com/matthewyjiang/rho/commit/b41ef92dcbeeba12351df4711f4817761fda0a79))
* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))
* **providers:** add Ollama Cloud API provider ([#597](https://github.com/matthewyjiang/rho/issues/597)) ([f6a62dd](https://github.com/matthewyjiang/rho/commit/f6a62ddb8c77bae1f6ba386328b79db625ec1e5d))
* **providers:** add Ollama support ([#466](https://github.com/matthewyjiang/rho/issues/466)) ([3a5a6d2](https://github.com/matthewyjiang/rho/commit/3a5a6d2fbf9fddcd87fbbb996e22438436a87823))
* **providers:** add Poolside API platform ([#483](https://github.com/matthewyjiang/rho/issues/483)) ([4684de7](https://github.com/matthewyjiang/rho/commit/4684de700f4312a90fa6d3173343a1dcfe7ef44d))
* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))
* **providers:** add xAI server-side context compaction ([#542](https://github.com/matthewyjiang/rho/issues/542)) ([2d43f13](https://github.com/matthewyjiang/rho/commit/2d43f134669414b3d9b7332a4c9d17aaa1346d9f))
* **providers:** let config name openai-compatible hosts ([#888](https://github.com/matthewyjiang/rho/issues/888)) ([a87649a](https://github.com/matthewyjiang/rho/commit/a87649a9e76332f53f125c52e7eccd8a14bc14f1))
* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **providers:** use provider-reported costs ([#455](https://github.com/matthewyjiang/rho/issues/455)) ([27a8c27](https://github.com/matthewyjiang/rho/commit/27a8c277b56a36a5c3da4e77041978db601f7a44))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))
* readmes for extracted library crates ([#388](https://github.com/matthewyjiang/rho/issues/388)) ([92c234d](https://github.com/matthewyjiang/rho/commit/92c234d6ef15ff85f7b68cb31ebdb479cb81f022))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tools:** replace edit_file with codex-style apply_patch ([#653](https://github.com/matthewyjiang/rho/issues/653)) ([eef1555](https://github.com/matthewyjiang/rho/commit/eef155521c5492b9c7f34507e82b6b7f46b896a8))
* **tui:** include subagent costs in status and info ([#548](https://github.com/matthewyjiang/rho/issues/548)) ([9517f00](https://github.com/matthewyjiang/rho/commit/9517f0012dd2001213fd10294287f8a0739e5d2c))
* **tui:** open or copy subagent attach from the activity rail ([#552](https://github.com/matthewyjiang/rho/issues/552)) ([95e4ca6](https://github.com/matthewyjiang/rho/commit/95e4ca698ab92f858587356ba16e29476bcfd972))
* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))
* **tui:** stream apply_patch diff cards ([#657](https://github.com/matthewyjiang/rho/issues/657)) ([e2c932e](https://github.com/matthewyjiang/rho/commit/e2c932e377f15ddfaab1e4700aa7d6f4e8ed0417))
* **tui:** unify tool transcript cards as Call + Children ([#586](https://github.com/matthewyjiang/rho/issues/586)) ([ce52cdd](https://github.com/matthewyjiang/rho/commit/ce52cddb6dbf0ac1b2878b6f3bd468a87547f8fa))
* **web:** prefer hosted search with backup provider config ([#649](https://github.com/matthewyjiang/rho/issues/649)) ([2e136e9](https://github.com/matthewyjiang/rho/commit/2e136e9025ebc318f7fac5da8e45a3134d785430))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* **agents:** yield while background work completes ([#396](https://github.com/matthewyjiang/rho/issues/396)) ([d54e9f3](https://github.com/matthewyjiang/rho/commit/d54e9f34d794f33bb493a3f0077582c6d37c4148))
* **auth:** prefer credential-backed auth and fix Ollama Cloud reasoning ([#619](https://github.com/matthewyjiang/rho/issues/619)) ([1a57f6f](https://github.com/matthewyjiang/rho/commit/1a57f6f24292b63a6ba4ba314843c2fb308792cf))
* **auth:** restore ollama device test key dir on unwind and isolate temp dir ([#621](https://github.com/matthewyjiang/rho/issues/621)) ([d2a345d](https://github.com/matthewyjiang/rho/commit/d2a345df56b1ae815007829a04cc21172500530d))
* **auth:** stop waiting for Ollama device callback ([#616](https://github.com/matthewyjiang/rho/issues/616)) ([54288d2](https://github.com/matthewyjiang/rho/commit/54288d28f7bcc68a36f0424e5de6c28e470fb479))
* **ci:** sync released tool dependency versions ([#391](https://github.com/matthewyjiang/rho/issues/391)) ([fc78948](https://github.com/matthewyjiang/rho/commit/fc78948953a790dcf6a8f783e67748cae0dd61dc))
* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **models:** reduce GPT-5.6 Codex context window ([#470](https://github.com/matthewyjiang/rho/issues/470)) ([2cf9cd6](https://github.com/matthewyjiang/rho/commit/2cf9cd6a74a1ab28798e08340b7ec2c731aab4f0))
* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))
* **openai:** handle terminal Codex websocket events ([#421](https://github.com/matthewyjiang/rho/issues/421)) ([c7fb4cd](https://github.com/matthewyjiang/rho/commit/c7fb4cdc1ae5db0ddb78589f03878643fa3df79d))
* **openai:** retry empty websocket responses ([#476](https://github.com/matthewyjiang/rho/issues/476)) ([04f3844](https://github.com/matthewyjiang/rho/commit/04f3844c79118e227b28bfee39b7af3f7c55b45e))
* **openai:** route gpt-5.6 Codex models through standard Responses ([#651](https://github.com/matthewyjiang/rho/issues/651)) ([219b9f5](https://github.com/matthewyjiang/rho/commit/219b9f593a42858bdbd47cac7d23ce224b81b84c))
* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))
* **prompt:** wait for catalog names before model labels ([#863](https://github.com/matthewyjiang/rho/issues/863)) ([71fa544](https://github.com/matthewyjiang/rho/commit/71fa544e86e5dd046b898be76632996d92915c19))
* **providers:** disable parallel tools on codex responses lite ([#583](https://github.com/matthewyjiang/rho/issues/583)) ([84ca3f5](https://github.com/matthewyjiang/rho/commit/84ca3f5ff0e6d535f40ebf92594e5c60df70a711))
* **providers:** enrich empty SSE content diagnostic ([#684](https://github.com/matthewyjiang/rho/issues/684)) ([79e4d48](https://github.com/matthewyjiang/rho/commit/79e4d48c735de05e1ceccde7cd3ae72a8e31e62f))
* **providers:** keep anthropic tool schemas typed after composition strip ([#753](https://github.com/matthewyjiang/rho/issues/753)) ([207e74c](https://github.com/matthewyjiang/rho/commit/207e74cc577d4c7c905f9bcf6b6b49e7153c9db5))
* **providers:** retry codex server_is_overloaded as unavailable ([#641](https://github.com/matthewyjiang/rho/issues/641)) ([9bb2c12](https://github.com/matthewyjiang/rho/commit/9bb2c124c758a2ee6bc4b8deb8d8b502f6145ff7)), closes [#639](https://github.com/matthewyjiang/rho/issues/639)
* **providers:** retry transient Gemini finish reasons instead of failing permanently ([#449](https://github.com/matthewyjiang/rho/issues/449)) ([041ea9d](https://github.com/matthewyjiang/rho/commit/041ea9deb6a98f4b1181d63f246d8ecc6b117609))
* **providers:** source Meta Muse Spark reasoning from models.dev ([#758](https://github.com/matthewyjiang/rho/issues/758)) ([a11eea0](https://github.com/matthewyjiang/rho/commit/a11eea0756cedfd2f1bb879603355c28a9c4a037))
* **providers:** stop inflated and deflated TPS for reasoning models ([#890](https://github.com/matthewyjiang/rho/issues/890)) ([a2c673d](https://github.com/matthewyjiang/rho/commit/a2c673dc48743e77689a67e4852fb6d3d094bc47))
* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))
* **release:** align agent tools dependencies ([#408](https://github.com/matthewyjiang/rho/issues/408)) ([ba07a4f](https://github.com/matthewyjiang/rho/commit/ba07a4ff95a19abef4898b8c9e410811f4ca9fcc))
* **release:** align dependent tool versions ([#426](https://github.com/matthewyjiang/rho/issues/426)) ([7b9ea52](https://github.com/matthewyjiang/rho/commit/7b9ea5211419bd600000466a0aab2d3d0405cda8))
* **release:** validate internal crate versions ([#395](https://github.com/matthewyjiang/rho/issues/395)) ([8f90461](https://github.com/matthewyjiang/rho/commit/8f90461f705ac57f1a53370c4f5de3711320c0e6))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** improve agent tool displays ([#413](https://github.com/matthewyjiang/rho/issues/413)) ([062edd0](https://github.com/matthewyjiang/rho/commit/062edd0851848c4fbd7754b47ec5dd588605989f))
* **tui:** lead approval prompts with the command ([#846](https://github.com/matthewyjiang/rho/issues/846)) ([413063c](https://github.com/matthewyjiang/rho/commit/413063c9d169e10ca369d83172fbbb952619f07c))
* **tui:** render narrow mermaid flowcharts and explain fallbacks ([#565](https://github.com/matthewyjiang/rho/issues/565)) ([0bf7ad7](https://github.com/matthewyjiang/rho/commit/0bf7ad719fa32d00bc6d1bc7857307032fd9f1f6))
* **tui:** reuse tool stream previews and allow codex parallel tools ([#566](https://github.com/matthewyjiang/rho/issues/566)) ([fa0074a](https://github.com/matthewyjiang/rho/commit/fa0074ae125972ac533ae09b30915f7e479674bd))
* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))
* **tui:** sort slash commands and provider pickers alphabetically ([#498](https://github.com/matthewyjiang/rho/issues/498)) ([0e2c16c](https://github.com/matthewyjiang/rho/commit/0e2c16cd9b5ac6b5c9c28259a09c0428f64a72ab))
* **tui:** wait for delegated goal work ([#457](https://github.com/matthewyjiang/rho/issues/457)) ([fc6087d](https://github.com/matthewyjiang/rho/commit/fc6087d4dfcbba2f3b82c5c9c0387dc31a59ab0b))
* **usage:** normalize cache write token accounting ([#511](https://github.com/matthewyjiang/rho/issues/511)) ([4e15982](https://github.com/matthewyjiang/rho/commit/4e15982a1e6f4738d40611d77c721ac26051bfda))
* **xai:** add grok-4.6 to the static model allowlist ([#882](https://github.com/matthewyjiang/rho/issues/882)) ([52ebd71](https://github.com/matthewyjiang/rho/commit/52ebd716456edcd9bd41d44d35afc7fc283cb31c))
* **xai:** keep optional grok Off as wire none ([#883](https://github.com/matthewyjiang/rho/issues/883)) ([e92f0a9](https://github.com/matthewyjiang/rho/commit/e92f0a96d5d53e61d295f8ac5be4a887fcb0a8f5))

## [1.1.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v1.1.0...rho-providers-v1.1.1) (2026-08-12)


### Bug Fixes

* **xai:** add grok-4.6 to the static model allowlist ([#882](https://github.com/matthewyjiang/rho/issues/882)) ([52ebd71](https://github.com/matthewyjiang/rho/commit/52ebd716456edcd9bd41d44d35afc7fc283cb31c))
* **xai:** keep optional grok Off as wire none ([#883](https://github.com/matthewyjiang/rho/issues/883)) ([e92f0a9](https://github.com/matthewyjiang/rho/commit/e92f0a96d5d53e61d295f8ac5be4a887fcb0a8f5))

## [1.1.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v1.0.0...rho-providers-v1.1.0) (2026-08-12)


### Features

* **permission:** rename Auto to Bypass and add classifier Auto ([#870](https://github.com/matthewyjiang/rho/issues/870)) ([3192daa](https://github.com/matthewyjiang/rho/commit/3192daa713f7202f44727ec4acb83d0c646d1286))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 3.1.0 to 4.0.0

## [1.0.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.21.0...rho-providers-v1.0.0) (2026-08-11)


### ⚠ BREAKING CHANGES

* **xai:** ModelEvent::WebSearch and RunEvent::WebSearch now carry a name field so hosts can distinguish web_search from x_search.

### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **agents:** add Claude Code subagent runtime ([#541](https://github.com/matthewyjiang/rho/issues/541)) ([c1385ec](https://github.com/matthewyjiang/rho/commit/c1385ecae9b2eb967ae73ecc09c20cc80bc63479))
* **agents:** allow pinning auth profiles on rho agents ([#781](https://github.com/matthewyjiang/rho/issues/781)) ([3e1f691](https://github.com/matthewyjiang/rho/commit/3e1f691e693dd93bf888cb5c3eb3093a7169525a))
* **agents:** move background-run contract into tool descriptions ([#405](https://github.com/matthewyjiang/rho/issues/405)) ([b75d0fa](https://github.com/matthewyjiang/rho/commit/b75d0fac659cd85a5469ce962e2bd026c673e288))
* **auth:** add active auth mode switcher ([#609](https://github.com/matthewyjiang/rho/issues/609)) ([a2b0f68](https://github.com/matthewyjiang/rho/commit/a2b0f68f71033ca6f8594a35368a79b2388916ca))
* **auth:** add configurable credential storage ([#478](https://github.com/matthewyjiang/rho/issues/478)) ([e778eda](https://github.com/matthewyjiang/rho/commit/e778edab71ec7e3c2f21137760f53bd0b8089469))
* **auth:** add OpenRouter OAuth login ([#472](https://github.com/matthewyjiang/rho/issues/472)) ([42af8e7](https://github.com/matthewyjiang/rho/commit/42af8e7a95bc1d16245f89dd1ebe74e6c4f56b7b))
* **cli:** add structured run output ([#467](https://github.com/matthewyjiang/rho/issues/467)) ([c4088bb](https://github.com/matthewyjiang/rho/commit/c4088bb03ef0e7e1b69de5e671773399755fe07b))
* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **prompt:** tell the agent which model runs it, its subagents, and the advisor ([#860](https://github.com/matthewyjiang/rho/issues/860)) ([d18c377](https://github.com/matthewyjiang/rho/commit/d18c3774a20657cc1214e2936251cc993b69dd14))
* **providers:** add Meta Model API and collapse provider registration ([#755](https://github.com/matthewyjiang/rho/issues/755)) ([b41ef92](https://github.com/matthewyjiang/rho/commit/b41ef92dcbeeba12351df4711f4817761fda0a79))
* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))
* **providers:** add Ollama Cloud API provider ([#597](https://github.com/matthewyjiang/rho/issues/597)) ([f6a62dd](https://github.com/matthewyjiang/rho/commit/f6a62ddb8c77bae1f6ba386328b79db625ec1e5d))
* **providers:** add Ollama support ([#466](https://github.com/matthewyjiang/rho/issues/466)) ([3a5a6d2](https://github.com/matthewyjiang/rho/commit/3a5a6d2fbf9fddcd87fbbb996e22438436a87823))
* **providers:** add Poolside API platform ([#483](https://github.com/matthewyjiang/rho/issues/483)) ([4684de7](https://github.com/matthewyjiang/rho/commit/4684de700f4312a90fa6d3173343a1dcfe7ef44d))
* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))
* **providers:** add xAI server-side context compaction ([#542](https://github.com/matthewyjiang/rho/issues/542)) ([2d43f13](https://github.com/matthewyjiang/rho/commit/2d43f134669414b3d9b7332a4c9d17aaa1346d9f))
* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **providers:** use provider-reported costs ([#455](https://github.com/matthewyjiang/rho/issues/455)) ([27a8c27](https://github.com/matthewyjiang/rho/commit/27a8c277b56a36a5c3da4e77041978db601f7a44))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))
* readmes for extracted library crates ([#388](https://github.com/matthewyjiang/rho/issues/388)) ([92c234d](https://github.com/matthewyjiang/rho/commit/92c234d6ef15ff85f7b68cb31ebdb479cb81f022))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tools:** replace edit_file with codex-style apply_patch ([#653](https://github.com/matthewyjiang/rho/issues/653)) ([eef1555](https://github.com/matthewyjiang/rho/commit/eef155521c5492b9c7f34507e82b6b7f46b896a8))
* **tui:** include subagent costs in status and info ([#548](https://github.com/matthewyjiang/rho/issues/548)) ([9517f00](https://github.com/matthewyjiang/rho/commit/9517f0012dd2001213fd10294287f8a0739e5d2c))
* **tui:** open or copy subagent attach from the activity rail ([#552](https://github.com/matthewyjiang/rho/issues/552)) ([95e4ca6](https://github.com/matthewyjiang/rho/commit/95e4ca698ab92f858587356ba16e29476bcfd972))
* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))
* **tui:** stream apply_patch diff cards ([#657](https://github.com/matthewyjiang/rho/issues/657)) ([e2c932e](https://github.com/matthewyjiang/rho/commit/e2c932e377f15ddfaab1e4700aa7d6f4e8ed0417))
* **tui:** unify tool transcript cards as Call + Children ([#586](https://github.com/matthewyjiang/rho/issues/586)) ([ce52cdd](https://github.com/matthewyjiang/rho/commit/ce52cddb6dbf0ac1b2878b6f3bd468a87547f8fa))
* **web:** prefer hosted search with backup provider config ([#649](https://github.com/matthewyjiang/rho/issues/649)) ([2e136e9](https://github.com/matthewyjiang/rho/commit/2e136e9025ebc318f7fac5da8e45a3134d785430))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* **agents:** yield while background work completes ([#396](https://github.com/matthewyjiang/rho/issues/396)) ([d54e9f3](https://github.com/matthewyjiang/rho/commit/d54e9f34d794f33bb493a3f0077582c6d37c4148))
* **auth:** prefer credential-backed auth and fix Ollama Cloud reasoning ([#619](https://github.com/matthewyjiang/rho/issues/619)) ([1a57f6f](https://github.com/matthewyjiang/rho/commit/1a57f6f24292b63a6ba4ba314843c2fb308792cf))
* **auth:** restore ollama device test key dir on unwind and isolate temp dir ([#621](https://github.com/matthewyjiang/rho/issues/621)) ([d2a345d](https://github.com/matthewyjiang/rho/commit/d2a345df56b1ae815007829a04cc21172500530d))
* **auth:** stop waiting for Ollama device callback ([#616](https://github.com/matthewyjiang/rho/issues/616)) ([54288d2](https://github.com/matthewyjiang/rho/commit/54288d28f7bcc68a36f0424e5de6c28e470fb479))
* **ci:** sync released tool dependency versions ([#391](https://github.com/matthewyjiang/rho/issues/391)) ([fc78948](https://github.com/matthewyjiang/rho/commit/fc78948953a790dcf6a8f783e67748cae0dd61dc))
* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **models:** reduce GPT-5.6 Codex context window ([#470](https://github.com/matthewyjiang/rho/issues/470)) ([2cf9cd6](https://github.com/matthewyjiang/rho/commit/2cf9cd6a74a1ab28798e08340b7ec2c731aab4f0))
* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))
* **openai:** handle terminal Codex websocket events ([#421](https://github.com/matthewyjiang/rho/issues/421)) ([c7fb4cd](https://github.com/matthewyjiang/rho/commit/c7fb4cdc1ae5db0ddb78589f03878643fa3df79d))
* **openai:** retry empty websocket responses ([#476](https://github.com/matthewyjiang/rho/issues/476)) ([04f3844](https://github.com/matthewyjiang/rho/commit/04f3844c79118e227b28bfee39b7af3f7c55b45e))
* **openai:** route gpt-5.6 Codex models through standard Responses ([#651](https://github.com/matthewyjiang/rho/issues/651)) ([219b9f5](https://github.com/matthewyjiang/rho/commit/219b9f593a42858bdbd47cac7d23ce224b81b84c))
* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))
* **prompt:** wait for catalog names before model labels ([#863](https://github.com/matthewyjiang/rho/issues/863)) ([71fa544](https://github.com/matthewyjiang/rho/commit/71fa544e86e5dd046b898be76632996d92915c19))
* **providers:** disable parallel tools on codex responses lite ([#583](https://github.com/matthewyjiang/rho/issues/583)) ([84ca3f5](https://github.com/matthewyjiang/rho/commit/84ca3f5ff0e6d535f40ebf92594e5c60df70a711))
* **providers:** enrich empty SSE content diagnostic ([#684](https://github.com/matthewyjiang/rho/issues/684)) ([79e4d48](https://github.com/matthewyjiang/rho/commit/79e4d48c735de05e1ceccde7cd3ae72a8e31e62f))
* **providers:** keep anthropic tool schemas typed after composition strip ([#753](https://github.com/matthewyjiang/rho/issues/753)) ([207e74c](https://github.com/matthewyjiang/rho/commit/207e74cc577d4c7c905f9bcf6b6b49e7153c9db5))
* **providers:** retry codex server_is_overloaded as unavailable ([#641](https://github.com/matthewyjiang/rho/issues/641)) ([9bb2c12](https://github.com/matthewyjiang/rho/commit/9bb2c124c758a2ee6bc4b8deb8d8b502f6145ff7)), closes [#639](https://github.com/matthewyjiang/rho/issues/639)
* **providers:** retry transient Gemini finish reasons instead of failing permanently ([#449](https://github.com/matthewyjiang/rho/issues/449)) ([041ea9d](https://github.com/matthewyjiang/rho/commit/041ea9deb6a98f4b1181d63f246d8ecc6b117609))
* **providers:** source Meta Muse Spark reasoning from models.dev ([#758](https://github.com/matthewyjiang/rho/issues/758)) ([a11eea0](https://github.com/matthewyjiang/rho/commit/a11eea0756cedfd2f1bb879603355c28a9c4a037))
* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))
* **release:** align agent tools dependencies ([#408](https://github.com/matthewyjiang/rho/issues/408)) ([ba07a4f](https://github.com/matthewyjiang/rho/commit/ba07a4ff95a19abef4898b8c9e410811f4ca9fcc))
* **release:** align dependent tool versions ([#426](https://github.com/matthewyjiang/rho/issues/426)) ([7b9ea52](https://github.com/matthewyjiang/rho/commit/7b9ea5211419bd600000466a0aab2d3d0405cda8))
* **release:** validate internal crate versions ([#395](https://github.com/matthewyjiang/rho/issues/395)) ([8f90461](https://github.com/matthewyjiang/rho/commit/8f90461f705ac57f1a53370c4f5de3711320c0e6))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** improve agent tool displays ([#413](https://github.com/matthewyjiang/rho/issues/413)) ([062edd0](https://github.com/matthewyjiang/rho/commit/062edd0851848c4fbd7754b47ec5dd588605989f))
* **tui:** lead approval prompts with the command ([#846](https://github.com/matthewyjiang/rho/issues/846)) ([413063c](https://github.com/matthewyjiang/rho/commit/413063c9d169e10ca369d83172fbbb952619f07c))
* **tui:** render narrow mermaid flowcharts and explain fallbacks ([#565](https://github.com/matthewyjiang/rho/issues/565)) ([0bf7ad7](https://github.com/matthewyjiang/rho/commit/0bf7ad719fa32d00bc6d1bc7857307032fd9f1f6))
* **tui:** reuse tool stream previews and allow codex parallel tools ([#566](https://github.com/matthewyjiang/rho/issues/566)) ([fa0074a](https://github.com/matthewyjiang/rho/commit/fa0074ae125972ac533ae09b30915f7e479674bd))
* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))
* **tui:** sort slash commands and provider pickers alphabetically ([#498](https://github.com/matthewyjiang/rho/issues/498)) ([0e2c16c](https://github.com/matthewyjiang/rho/commit/0e2c16cd9b5ac6b5c9c28259a09c0428f64a72ab))
* **tui:** wait for delegated goal work ([#457](https://github.com/matthewyjiang/rho/issues/457)) ([fc6087d](https://github.com/matthewyjiang/rho/commit/fc6087d4dfcbba2f3b82c5c9c0387dc31a59ab0b))
* **usage:** normalize cache write token accounting ([#511](https://github.com/matthewyjiang/rho/issues/511)) ([4e15982](https://github.com/matthewyjiang/rho/commit/4e15982a1e6f4738d40611d77c721ac26051bfda))

## [0.20.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.19.1...rho-providers-v0.20.0) (2026-08-11)


### Features

* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 2.1.0 to 3.0.0

## [0.19.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.19.0...rho-providers-v0.19.1) (2026-08-10)


### Bug Fixes

* **tui:** lead approval prompts with the command ([#846](https://github.com/matthewyjiang/rho/issues/846)) ([413063c](https://github.com/matthewyjiang/rho/commit/413063c9d169e10ca369d83172fbbb952619f07c))

## [0.19.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.18.2...rho-providers-v0.19.0) (2026-08-10)


### Features

* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.18.0 to 2.0.0

## [0.18.2](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.18.1...rho-providers-v0.18.2) (2026-08-08)


### Bug Fixes

* exclude reasoning tokens from throughput ([#819](https://github.com/matthewyjiang/rho/issues/819)) ([d261b5b](https://github.com/matthewyjiang/rho/commit/d261b5b35bfb119f49a81d83b33ca06b62b383e7))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.2 to 1.17.3

## [0.18.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.18.0...rho-providers-v0.18.1) (2026-08-07)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.1 to 1.17.2

## [0.18.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.17.1...rho-providers-v0.18.0) (2026-08-07)


### Features

* **agents:** allow pinning auth profiles on rho agents ([#781](https://github.com/matthewyjiang/rho/issues/781)) ([3e1f691](https://github.com/matthewyjiang/rho/commit/3e1f691e693dd93bf888cb5c3eb3093a7169525a))


### Bug Fixes

* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.0 to 1.17.1

## [0.17.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.17.0...rho-providers-v0.17.1) (2026-08-06)


### Bug Fixes

* **providers:** source Meta Muse Spark reasoning from models.dev ([#758](https://github.com/matthewyjiang/rho/issues/758)) ([a11eea0](https://github.com/matthewyjiang/rho/commit/a11eea0756cedfd2f1bb879603355c28a9c4a037))

## [0.17.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.16.1...rho-providers-v0.17.0) (2026-08-06)


### Features

* add advisor mode with a selectable advisor model ([#752](https://github.com/matthewyjiang/rho/issues/752)) ([13c1ebb](https://github.com/matthewyjiang/rho/commit/13c1ebb89edfde2924ee760c7621b099fd510708))
* **providers:** add Meta Model API and collapse provider registration ([#755](https://github.com/matthewyjiang/rho/issues/755)) ([b41ef92](https://github.com/matthewyjiang/rho/commit/b41ef92dcbeeba12351df4711f4817761fda0a79))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.16.0 to 1.17.0

## [0.16.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.16.0...rho-providers-v0.16.1) (2026-08-05)


### Bug Fixes

* **providers:** keep anthropic tool schemas typed after composition strip ([#753](https://github.com/matthewyjiang/rho/issues/753)) ([207e74c](https://github.com/matthewyjiang/rho/commit/207e74cc577d4c7c905f9bcf6b6b49e7153c9db5))

## [0.16.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.15.5...rho-providers-v0.16.0) (2026-08-04)


### Features

* **providers:** add Qwen Token Plan OpenAI-compatible provider ([#738](https://github.com/matthewyjiang/rho/issues/738)) ([6aa6df2](https://github.com/matthewyjiang/rho/commit/6aa6df2e812674b721bedd3b65c7c2cdb359a1e4))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.2 to 1.16.0

## [0.15.5](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.15.4...rho-providers-v0.15.5) (2026-08-04)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.1 to 1.15.2

## [0.15.4](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.15.3...rho-providers-v0.15.4) (2026-08-03)


### Bug Fixes

* **providers:** surface rate-limit reset time and /limits pointer ([#733](https://github.com/matthewyjiang/rho/issues/733)) ([b9371fc](https://github.com/matthewyjiang/rho/commit/b9371fc69fb9b195f9f400d872195c91f031a6b2))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.0 to 1.15.1

## [0.15.3](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.15.2...rho-providers-v0.15.3) (2026-08-02)


### Bug Fixes

* **providers:** enrich empty SSE content diagnostic ([#684](https://github.com/matthewyjiang/rho/issues/684)) ([79e4d48](https://github.com/matthewyjiang/rho/commit/79e4d48c735de05e1ceccde7cd3ae72a8e31e62f))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.14.0 to 1.15.0

## [0.15.2](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.15.1...rho-providers-v0.15.2) (2026-07-31)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.1 to 1.14.0

## [0.15.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.15.0...rho-providers-v0.15.1) (2026-07-30)


### Bug Fixes

* **tui:** show codex fast mode and report tier fallback ([#663](https://github.com/matthewyjiang/rho/issues/663)) ([177043f](https://github.com/matthewyjiang/rho/commit/177043f5022a1798ae45b0d987e6c6ceaf470d1c))
* **tui:** show hosted x_search tool cards ([#662](https://github.com/matthewyjiang/rho/issues/662)) ([4381667](https://github.com/matthewyjiang/rho/commit/438166754b79645d31b4fcefd92b3ea665567c94))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.0 to 1.13.1

## [0.15.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.14.0...rho-providers-v0.15.0) (2026-07-30)


### Features

* **tui:** stream apply_patch diff cards ([#657](https://github.com/matthewyjiang/rho/issues/657)) ([e2c932e](https://github.com/matthewyjiang/rho/commit/e2c932e377f15ddfaab1e4700aa7d6f4e8ed0417))

## [0.14.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.13.3...rho-providers-v0.14.0) (2026-07-30)


### Features

* **tools:** replace edit_file with codex-style apply_patch ([#653](https://github.com/matthewyjiang/rho/issues/653)) ([eef1555](https://github.com/matthewyjiang/rho/commit/eef155521c5492b9c7f34507e82b6b7f46b896a8))
* **web:** prefer hosted search with backup provider config ([#649](https://github.com/matthewyjiang/rho/issues/649)) ([2e136e9](https://github.com/matthewyjiang/rho/commit/2e136e9025ebc318f7fac5da8e45a3134d785430))
* **xai:** support hosted x_search tool ([#647](https://github.com/matthewyjiang/rho/issues/647)) ([cd0c897](https://github.com/matthewyjiang/rho/commit/cd0c897570376cf39d2d99b40c58c55b22fc6133))


### Bug Fixes

* **openai:** align Codex Responses wire contracts ([#644](https://github.com/matthewyjiang/rho/issues/644)) ([76cf855](https://github.com/matthewyjiang/rho/commit/76cf8554c390dfa112801016f2c05bd929c35eee))
* **openai:** route gpt-5.6 Codex models through standard Responses ([#651](https://github.com/matthewyjiang/rho/issues/651)) ([219b9f5](https://github.com/matthewyjiang/rho/commit/219b9f593a42858bdbd47cac7d23ce224b81b84c))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.2 to 1.13.0

## [0.13.3](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.13.2...rho-providers-v0.13.3) (2026-07-29)


### Bug Fixes

* **providers:** retry codex server_is_overloaded as unavailable ([#641](https://github.com/matthewyjiang/rho/issues/641)) ([9bb2c12](https://github.com/matthewyjiang/rho/commit/9bb2c124c758a2ee6bc4b8deb8d8b502f6145ff7)), closes [#639](https://github.com/matthewyjiang/rho/issues/639)

## [0.13.2](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.13.1...rho-providers-v0.13.2) (2026-07-29)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.1 to 1.12.2

## [0.13.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.13.0...rho-providers-v0.13.1) (2026-07-28)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.0 to 1.12.1

## [0.13.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.12.2...rho-providers-v0.13.0) (2026-07-28)


### Features

* **tui:** show model output token rate ([#623](https://github.com/matthewyjiang/rho/issues/623)) ([a5aa688](https://github.com/matthewyjiang/rho/commit/a5aa688686d9f4f08d064462ccfa4fd542aa979d))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.11.0 to 1.12.0

## [0.12.2](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.12.1...rho-providers-v0.12.2) (2026-07-28)


### Bug Fixes

* **auth:** prefer credential-backed auth and fix Ollama Cloud reasoning ([#619](https://github.com/matthewyjiang/rho/issues/619)) ([1a57f6f](https://github.com/matthewyjiang/rho/commit/1a57f6f24292b63a6ba4ba314843c2fb308792cf))
* **auth:** restore ollama device test key dir on unwind and isolate temp dir ([#621](https://github.com/matthewyjiang/rho/issues/621)) ([d2a345d](https://github.com/matthewyjiang/rho/commit/d2a345df56b1ae815007829a04cc21172500530d))

## [0.12.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.12.0...rho-providers-v0.12.1) (2026-07-28)


### Bug Fixes

* **auth:** stop waiting for Ollama device callback ([#616](https://github.com/matthewyjiang/rho/issues/616)) ([54288d2](https://github.com/matthewyjiang/rho/commit/54288d28f7bcc68a36f0424e5de6c28e470fb479))

## [0.12.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.11.1...rho-providers-v0.12.0) (2026-07-28)


### Features

* **auth:** add active auth mode switcher ([#609](https://github.com/matthewyjiang/rho/issues/609)) ([a2b0f68](https://github.com/matthewyjiang/rho/commit/a2b0f68f71033ca6f8594a35368a79b2388916ca))
* **openai:** add fast mode ([#610](https://github.com/matthewyjiang/rho/issues/610)) ([8c5cd6d](https://github.com/matthewyjiang/rho/commit/8c5cd6d19e1758b85fc25c345769e49426f10ad0))
* **providers:** add Ollama Cloud API provider ([#597](https://github.com/matthewyjiang/rho/issues/597)) ([f6a62dd](https://github.com/matthewyjiang/rho/commit/f6a62ddb8c77bae1f6ba386328b79db625ec1e5d))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.3 to 1.11.0

## [0.11.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.11.0...rho-providers-v0.11.1) (2026-07-27)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.2 to 1.10.3

## [0.11.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.10.2...rho-providers-v0.11.0) (2026-07-27)


### Features

* **tui:** unify tool transcript cards as Call + Children ([#586](https://github.com/matthewyjiang/rho/issues/586)) ([ce52cdd](https://github.com/matthewyjiang/rho/commit/ce52cddb6dbf0ac1b2878b6f3bd468a87547f8fa))

## [0.10.2](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.10.1...rho-providers-v0.10.2) (2026-07-27)


### Bug Fixes

* **providers:** disable parallel tools on codex responses lite ([#583](https://github.com/matthewyjiang/rho/issues/583)) ([84ca3f5](https://github.com/matthewyjiang/rho/commit/84ca3f5ff0e6d535f40ebf92594e5c60df70a711))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.1 to 1.10.2

## [0.10.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.10.0...rho-providers-v0.10.1) (2026-07-26)


### Bug Fixes

* **tui:** render narrow mermaid flowcharts and explain fallbacks ([#565](https://github.com/matthewyjiang/rho/issues/565)) ([0bf7ad7](https://github.com/matthewyjiang/rho/commit/0bf7ad719fa32d00bc6d1bc7857307032fd9f1f6))
* **tui:** reuse tool stream previews and allow codex parallel tools ([#566](https://github.com/matthewyjiang/rho/issues/566)) ([fa0074a](https://github.com/matthewyjiang/rho/commit/fa0074ae125972ac533ae09b30915f7e479674bd))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.0 to 1.10.1

## [0.10.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.9.0...rho-providers-v0.10.0) (2026-07-26)


### Features

* **tui:** open or copy subagent attach from the activity rail ([#552](https://github.com/matthewyjiang/rho/issues/552)) ([95e4ca6](https://github.com/matthewyjiang/rho/commit/95e4ca698ab92f858587356ba16e29476bcfd972))

## [0.9.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.8.1...rho-providers-v0.9.0) (2026-07-25)


### Features

* **agents:** add Claude Code subagent runtime ([#541](https://github.com/matthewyjiang/rho/issues/541)) ([c1385ec](https://github.com/matthewyjiang/rho/commit/c1385ecae9b2eb967ae73ecc09c20cc80bc63479))
* **providers:** add xAI server-side context compaction ([#542](https://github.com/matthewyjiang/rho/issues/542)) ([2d43f13](https://github.com/matthewyjiang/rho/commit/2d43f134669414b3d9b7332a4c9d17aaa1346d9f))
* **subagents:** route background questionnaires to parent ([#539](https://github.com/matthewyjiang/rho/issues/539)) ([e0cab31](https://github.com/matthewyjiang/rho/commit/e0cab3182e9fc833fbf304c7dad5714f73b89952))
* **tui:** include subagent costs in status and info ([#548](https://github.com/matthewyjiang/rho/issues/548)) ([9517f00](https://github.com/matthewyjiang/rho/commit/9517f0012dd2001213fd10294287f8a0739e5d2c))


### Bug Fixes

* **tools:** allow file paths outside workspace ([#537](https://github.com/matthewyjiang/rho/issues/537)) ([8a3cc24](https://github.com/matthewyjiang/rho/commit/8a3cc24468e89bb509fefbefced738b706b1e43d))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.9.0 to 1.10.0

## [0.8.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.8.0...rho-providers-v0.8.1) (2026-07-24)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.8.0 to 1.9.0

## [0.8.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.7.1...rho-providers-v0.8.0) (2026-07-23)


### Features

* **providers:** use OpenAI server-side compaction for codex and api-key ([#514](https://github.com/matthewyjiang/rho/issues/514)) ([b18eadd](https://github.com/matthewyjiang/rho/commit/b18eadd6752de2945361cd59a60ffc4cc7b807ad))
* **questionnaire:** support choice descriptions ([#510](https://github.com/matthewyjiang/rho/issues/510)) ([066899c](https://github.com/matthewyjiang/rho/commit/066899c2ad12ca23c2b7772de4b0a6a3c6161497))


### Bug Fixes

* **poolside:** publish final stream usage snapshot ([#516](https://github.com/matthewyjiang/rho/issues/516)) ([d51ebab](https://github.com/matthewyjiang/rho/commit/d51ebabcc4823ef11b21b8fadecd6625956146d2))
* **usage:** normalize cache write token accounting ([#511](https://github.com/matthewyjiang/rho/issues/511)) ([4e15982](https://github.com/matthewyjiang/rho/commit/4e15982a1e6f4738d40611d77c721ac26051bfda))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.2 to 1.8.0

## [0.7.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.7.0...rho-providers-v0.7.1) (2026-07-22)


### Bug Fixes

* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** sort slash commands and provider pickers alphabetically ([#498](https://github.com/matthewyjiang/rho/issues/498)) ([0e2c16c](https://github.com/matthewyjiang/rho/commit/0e2c16cd9b5ac6b5c9c28259a09c0428f64a72ab))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.1 to 1.7.2

## [0.7.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.6.0...rho-providers-v0.7.0) (2026-07-22)


### Features

* **providers:** add Poolside API platform ([#483](https://github.com/matthewyjiang/rho/issues/483)) ([4684de7](https://github.com/matthewyjiang/rho/commit/4684de700f4312a90fa6d3173343a1dcfe7ef44d))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.0 to 1.7.1

## [0.6.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.5.0...rho-providers-v0.6.0) (2026-07-22)


### Features

* **auth:** add configurable credential storage ([#478](https://github.com/matthewyjiang/rho/issues/478)) ([e778eda](https://github.com/matthewyjiang/rho/commit/e778edab71ec7e3c2f21137760f53bd0b8089469))
* **auth:** add OpenRouter OAuth login ([#472](https://github.com/matthewyjiang/rho/issues/472)) ([42af8e7](https://github.com/matthewyjiang/rho/commit/42af8e7a95bc1d16245f89dd1ebe74e6c4f56b7b))
* **cli:** add structured run output ([#467](https://github.com/matthewyjiang/rho/issues/467)) ([c4088bb](https://github.com/matthewyjiang/rho/commit/c4088bb03ef0e7e1b69de5e671773399755fe07b))
* **providers:** add Ollama support ([#466](https://github.com/matthewyjiang/rho/issues/466)) ([3a5a6d2](https://github.com/matthewyjiang/rho/commit/3a5a6d2fbf9fddcd87fbbb996e22438436a87823))


### Bug Fixes

* **models:** reduce GPT-5.6 Codex context window ([#470](https://github.com/matthewyjiang/rho/issues/470)) ([2cf9cd6](https://github.com/matthewyjiang/rho/commit/2cf9cd6a74a1ab28798e08340b7ec2c731aab4f0))
* **openai:** retry empty websocket responses ([#476](https://github.com/matthewyjiang/rho/issues/476)) ([04f3844](https://github.com/matthewyjiang/rho/commit/04f3844c79118e227b28bfee39b7af3f7c55b45e))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.6.0 to 1.7.0

## [0.5.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.4.0...rho-providers-v0.5.0) (2026-07-21)


### Features

* **providers:** use provider-reported costs ([#455](https://github.com/matthewyjiang/rho/issues/455)) ([27a8c27](https://github.com/matthewyjiang/rho/commit/27a8c277b56a36a5c3da4e77041978db601f7a44))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))


### Bug Fixes

* **providers:** retry transient Gemini finish reasons instead of failing permanently ([#449](https://github.com/matthewyjiang/rho/issues/449)) ([041ea9d](https://github.com/matthewyjiang/rho/commit/041ea9deb6a98f4b1181d63f246d8ecc6b117609))
* **skills:** enforce manual skill invocation ([#453](https://github.com/matthewyjiang/rho/issues/453)) ([4f6f043](https://github.com/matthewyjiang/rho/commit/4f6f043026622fc46a8d93e4ee8b743ccb2a36ea))
* **tui:** wait for delegated goal work ([#457](https://github.com/matthewyjiang/rho/issues/457)) ([fc6087d](https://github.com/matthewyjiang/rho/commit/fc6087d4dfcbba2f3b82c5c9c0387dc31a59ab0b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.5.0 to 1.6.0

## [0.4.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.3.2...rho-providers-v0.4.0) (2026-07-20)


### Features

* **providers:** add native Google Gemini support ([#430](https://github.com/matthewyjiang/rho/issues/430)) ([34ef307](https://github.com/matthewyjiang/rho/commit/34ef3076d08afb9b1261973318e2173a7d14a613))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.4.0 to 1.5.0

## [0.3.2](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.3.1...rho-providers-v0.3.2) (2026-07-20)


### Bug Fixes

* **release:** align dependent tool versions ([#426](https://github.com/matthewyjiang/rho/issues/426)) ([7b9ea52](https://github.com/matthewyjiang/rho/commit/7b9ea5211419bd600000466a0aab2d3d0405cda8))

## [0.3.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.3.0...rho-providers-v0.3.1) (2026-07-20)


### Bug Fixes

* **openai:** handle terminal Codex websocket events ([#421](https://github.com/matthewyjiang/rho/issues/421)) ([c7fb4cd](https://github.com/matthewyjiang/rho/commit/c7fb4cdc1ae5db0ddb78589f03878643fa3df79d))
* **tui:** improve agent tool displays ([#413](https://github.com/matthewyjiang/rho/issues/413)) ([062edd0](https://github.com/matthewyjiang/rho/commit/062edd0851848c4fbd7754b47ec5dd588605989f))

## [0.3.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.2.1...rho-providers-v0.3.0) (2026-07-20)


### Features

* **agents:** move background-run contract into tool descriptions ([#405](https://github.com/matthewyjiang/rho/issues/405)) ([b75d0fa](https://github.com/matthewyjiang/rho/commit/b75d0fac659cd85a5469ce962e2bd026c673e288))


### Bug Fixes

* **agents:** yield while background work completes ([#396](https://github.com/matthewyjiang/rho/issues/396)) ([d54e9f3](https://github.com/matthewyjiang/rho/commit/d54e9f34d794f33bb493a3f0077582c6d37c4148))
* **kimi:** use provider-native K3 reasoning ([#402](https://github.com/matthewyjiang/rho/issues/402)) ([5453cdc](https://github.com/matthewyjiang/rho/commit/5453cdc5c78df2b11b3e5bbab4ea96c5fba635d9))
* **sdk:** retry retryable provider failures instead of failing the run ([#401](https://github.com/matthewyjiang/rho/issues/401)) ([b2867da](https://github.com/matthewyjiang/rho/commit/b2867da58eab9636c5e9691fe1de25e669a36dc3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.3.0 to 1.4.0

## [0.2.1](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.2.0...rho-providers-v0.2.1) (2026-07-18)


### Bug Fixes

* **ci:** sync released tool dependency versions ([#391](https://github.com/matthewyjiang/rho/issues/391)) ([fc78948](https://github.com/matthewyjiang/rho/commit/fc78948953a790dcf6a8f783e67748cae0dd61dc))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.1.0 to 1.3.0

## [0.2.0](https://github.com/matthewyjiang/rho/compare/rho-providers-v0.1.0...rho-providers-v0.2.0) (2026-07-18)


### Features

* readmes for extracted library crates ([#388](https://github.com/matthewyjiang/rho/issues/388)) ([92c234d](https://github.com/matthewyjiang/rho/commit/92c234d6ef15ff85f7b68cb31ebdb479cb81f022))
