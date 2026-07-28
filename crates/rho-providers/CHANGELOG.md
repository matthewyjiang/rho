# Changelog

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
