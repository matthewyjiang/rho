# Changelog

## [1.0.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v1.0.0...rho-agent-tools-v1.0.1) (2026-08-27)


### Bug Fixes

* **deps:** patch h2, lopdf, webbrowser, and quick-xml CVEs ([#1069](https://github.com/matthewyjiang/rho/issues/1069)) ([453be34](https://github.com/matthewyjiang/rho/commit/453be34aadd44faa3063bf813fd14007e95c2774))

## [1.0.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.20.1...rho-agent-tools-v1.0.0) (2026-08-27)


### ⚠ BREAKING CHANGES

* rho-sdk removes RunEvent::ProviderActivity and the PROVIDER_ACTIVITY_* constants, collapses ProviderStreamResetReason retryable variants into RetryableFailure { kind, retry_after }, removes ModelCallMetrics::output_tokens_per_second, adds revision to RunEvent::Failed, folds ContextEstimated into StepStarted, and replaces the stringly generation-output-token carriers with a typed ModelCallMetrics::generation_output_tokens metric and ModelEvent::GenerationOutputTokens variant. rho-providers adds ProviderId::OpenAiCompatible for config-defined hosts, folds CustomProviderOptions into CustomProviderSpec (removing the _with_lookup/ _with_options funnels), makes providers() include custom hosts (removing visible_providers), removes deprecated registry API-base re-exports and ProviderRuntime::same_family, and drops the CredentialStore parameter from OpenAiProvider::new_with_identity. rho-tools removes the edit_file tool-name alias, the write_file frontmatter alias, and the EditToolKind type alias.

### Code Refactoring

* pay down all NEXT_MAJOR debt for the next major, net -986 lines ([#1062](https://github.com/matthewyjiang/rho/issues/1062)) ([0e17bdb](https://github.com/matthewyjiang/rho/commit/0e17bdb5e9fa21440fffbbe00171830ac0d1d3c2))

## [0.20.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.20.0...rho-agent-tools-v0.20.1) (2026-08-25)


### Performance Improvements

* cut hot-path costs in grep, session persistence, and live TUI rendering ([#1027](https://github.com/matthewyjiang/rho/issues/1027)) ([9cf4ad7](https://github.com/matthewyjiang/rho/commit/9cf4ad73149ebcaffbe760481618cac9bb02a061))

## [0.20.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.19.0...rho-agent-tools-v0.20.0) (2026-08-21)


### Features

* **acp:** let hosts set reasoning through thought_level ([#1015](https://github.com/matthewyjiang/rho/issues/1015)) ([5cda8e8](https://github.com/matthewyjiang/rho/commit/5cda8e8308682fa13c9d6001ec12989f0d030bb4))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 4.1.1 to 4.2.0

## [0.19.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.18.1...rho-agent-tools-v0.19.0) (2026-08-20)


### Features

* **tools:** mint hashline tags only for the hashline edit tool ([#1000](https://github.com/matthewyjiang/rho/issues/1000)) ([2fa3c2c](https://github.com/matthewyjiang/rho/commit/2fa3c2c35280f79cb53826cf3cc863421fcd18d0))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 4.1.0 to 4.1.1

## [0.18.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.18.0...rho-agent-tools-v0.18.1) (2026-08-18)


### Performance Improvements

* **tools:** cut tokens from web, process, and shell results ([#980](https://github.com/matthewyjiang/rho/issues/980)) ([c103ec0](https://github.com/matthewyjiang/rho/commit/c103ec0d6d46896f735074413369cc471c95a750))

## [0.18.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.17.1...rho-agent-tools-v0.18.0) (2026-08-17)


### Features

* **xai:** let grok generate and edit images in the conversation ([#967](https://github.com/matthewyjiang/rho/issues/967)) ([47c03c1](https://github.com/matthewyjiang/rho/commit/47c03c1d2b13864d32a69b27c21da4db54572e0e))

## [0.17.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.17.0...rho-agent-tools-v0.17.1) (2026-08-15)


### Performance Improvements

* **tools:** stream hashline digest, zero-alloc grep lines, and dirty-check shell stream ([#927](https://github.com/matthewyjiang/rho/issues/927)) ([580a774](https://github.com/matthewyjiang/rho/commit/580a7742e6f268240874bf0df4987353c90bf4b4))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 4.0.0 to 4.0.1

## [0.17.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.16.0...rho-agent-tools-v0.17.0) (2026-08-12)


### Features

* **permission:** rename Auto to Bypass and add classifier Auto ([#870](https://github.com/matthewyjiang/rho/issues/870)) ([3192daa](https://github.com/matthewyjiang/rho/commit/3192daa713f7202f44727ec4acb83d0c646d1286))
* **tui:** count up shell runtime next to timeout ([#871](https://github.com/matthewyjiang/rho/issues/871)) ([7c55124](https://github.com/matthewyjiang/rho/commit/7c55124aa43fb40db414c7be01ab68e36dfb0441))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 3.1.0 to 4.0.0

## [0.16.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.15.1...rho-agent-tools-v0.16.0) (2026-08-11)


### Features

* **subagents:** add parent-child plain-text messaging for Rho runtime ([#852](https://github.com/matthewyjiang/rho/issues/852)) ([dd25d8e](https://github.com/matthewyjiang/rho/commit/dd25d8e3e48fd531e777e31fcad9c948a2a9ebfe))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 2.1.0 to 3.0.0

## [0.15.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.15.0...rho-agent-tools-v0.15.1) (2026-08-10)


### Bug Fixes

* **tools:** reject shell timeout_seconds of zero consistently ([#843](https://github.com/matthewyjiang/rho/issues/843)) ([c379aec](https://github.com/matthewyjiang/rho/commit/c379aeca8ad0dd2ad53a33df66f967782206db78))

## [0.15.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.14.0...rho-agent-tools-v0.15.0) (2026-08-10)


### Features

* **config:** mid-session edit tool, advisor, and auto preference ([#840](https://github.com/matthewyjiang/rho/issues/840)) ([423d026](https://github.com/matthewyjiang/rho/commit/423d02690edee36a6dc692ac25d8fd9013d33139))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.18.0 to 2.0.0

## [0.14.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.13.0...rho-agent-tools-v0.14.0) (2026-08-08)


### Features

* **auth:** add configurable credential storage ([#478](https://github.com/matthewyjiang/rho/issues/478)) ([e778eda](https://github.com/matthewyjiang/rho/commit/e778edab71ec7e3c2f21137760f53bd0b8089469))
* **documents:** add bounded document extraction and attachments ([#669](https://github.com/matthewyjiang/rho/issues/669)) ([d1ec3cd](https://github.com/matthewyjiang/rho/commit/d1ec3cd5d8f5683c7b8de0047070a6029bb1ec33))
* readmes for extracted library crates ([#388](https://github.com/matthewyjiang/rho/issues/388)) ([92c234d](https://github.com/matthewyjiang/rho/commit/92c234d6ef15ff85f7b68cb31ebdb479cb81f022))
* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))
* **sessions:** add workspace rewind checkpoints ([#638](https://github.com/matthewyjiang/rho/issues/638)) ([5a90b2d](https://github.com/matthewyjiang/rho/commit/5a90b2db5b1170f2701cbac1c0c7d056f9158754))
* **tools:** add in-process grep and glob workspace tools ([#554](https://github.com/matthewyjiang/rho/issues/554)) ([e422a99](https://github.com/matthewyjiang/rho/commit/e422a990332afff330b096d8960d4e0fa07a5838))
* **tools:** add selectable edit formats ([#820](https://github.com/matthewyjiang/rho/issues/820)) ([5db37d0](https://github.com/matthewyjiang/rho/commit/5db37d0bf714afbf28e91e3f0e66c2e8b807b659))
* **tools:** extract pdf content with pdf-inspector ([#687](https://github.com/matthewyjiang/rho/issues/687)) ([ce92355](https://github.com/matthewyjiang/rho/commit/ce92355e74a56ab11d7f2cf35ff3d246e34310d2))
* **tools:** replace edit_file with codex-style apply_patch ([#653](https://github.com/matthewyjiang/rho/issues/653)) ([eef1555](https://github.com/matthewyjiang/rho/commit/eef155521c5492b9c7f34507e82b6b7f46b896a8))
* **tools:** restore simple edit_file ([#658](https://github.com/matthewyjiang/rho/issues/658)) ([ffac70f](https://github.com/matthewyjiang/rho/commit/ffac70f6d58d1532a4eedefbdc99463402adbf7b))
* **tui:** borderless code, mermaid, and math blocks with syntect highlighting ([#825](https://github.com/matthewyjiang/rho/issues/825)) ([f0614a8](https://github.com/matthewyjiang/rho/commit/f0614a88d035f0465adbddccc830207d5366450f))
* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))
* **tui:** stream apply_patch diff cards ([#657](https://github.com/matthewyjiang/rho/issues/657)) ([e2c932e](https://github.com/matthewyjiang/rho/commit/e2c932e377f15ddfaab1e4700aa7d6f4e8ed0417))
* **tui:** unify tool transcript cards as Call + Children ([#586](https://github.com/matthewyjiang/rho/issues/586)) ([ce52cdd](https://github.com/matthewyjiang/rho/commit/ce52cddb6dbf0ac1b2878b6f3bd468a87547f8fa))


### Bug Fixes

* **agents:** clarify foreground agent batch behavior ([#606](https://github.com/matthewyjiang/rho/issues/606)) ([9574e48](https://github.com/matthewyjiang/rho/commit/9574e4836a3c6e14eb28bc5863b8d2abc334e140))
* **errors:** surface failures that were silently swallowed ([#546](https://github.com/matthewyjiang/rho/issues/546)) ([1d4eee3](https://github.com/matthewyjiang/rho/commit/1d4eee3ea2e45d459897198d48babbe3ded3bf19))
* **release:** guard publishable crate version bumps ([#424](https://github.com/matthewyjiang/rho/issues/424)) ([4b39b58](https://github.com/matthewyjiang/rho/commit/4b39b58cb09a2815be4d5350c2b0e0a831a426fe))
* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))
* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))
* **tools:** bound the timeout drain so escaped processes cannot stall bash ([#342](https://github.com/matthewyjiang/rho/issues/342)) ([414850f](https://github.com/matthewyjiang/rho/commit/414850fd374315f85691323d6bcf5615880da0d2))
* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))
* **tui:** drop duplicate paths on multi-file edit cards ([#826](https://github.com/matthewyjiang/rho/issues/826)) ([16c98e3](https://github.com/matthewyjiang/rho/commit/16c98e3c1898cbb885e71366a940efeaa52aa688))
* **tui:** paste image paths and fall back kitty under herdr ([#504](https://github.com/matthewyjiang/rho/issues/504)) ([c140bfe](https://github.com/matthewyjiang/rho/commit/c140bfe6994f4ffc42756075ec801eff6e63ce40))
* **tui:** validate dropped file attachments ([#677](https://github.com/matthewyjiang/rho/issues/677)) ([1c62a3d](https://github.com/matthewyjiang/rho/commit/1c62a3d638328bc829350f1cf32649fd58f7abcb))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.2 to 1.17.3

## [0.12.6](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.12.5...rho-agent-tools-v0.12.6) (2026-08-07)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.1 to 1.17.2

## [0.12.5](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.12.4...rho-agent-tools-v0.12.5) (2026-08-07)


### Bug Fixes

* **sdk:** recover failed 1.32.0 release packaging ([#792](https://github.com/matthewyjiang/rho/issues/792)) ([a782145](https://github.com/matthewyjiang/rho/commit/a782145820f2924a47140f9e8cd8e3cbd13be8a3))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.17.0 to 1.17.1

## [0.12.4](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.12.3...rho-agent-tools-v0.12.4) (2026-08-06)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.16.0 to 1.17.0

## [0.12.3](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.12.2...rho-agent-tools-v0.12.3) (2026-08-04)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.2 to 1.16.0

## [0.12.2](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.12.1...rho-agent-tools-v0.12.2) (2026-08-04)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.1 to 1.15.2

## [0.12.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.12.0...rho-agent-tools-v0.12.1) (2026-08-03)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.15.0 to 1.15.1

## [0.12.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.11.1...rho-agent-tools-v0.12.0) (2026-08-02)


### Features

* **tools:** extract pdf content with pdf-inspector ([#687](https://github.com/matthewyjiang/rho/issues/687)) ([ce92355](https://github.com/matthewyjiang/rho/commit/ce92355e74a56ab11d7f2cf35ff3d246e34310d2))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.14.0 to 1.15.0

## [0.11.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.11.0...rho-agent-tools-v0.11.1) (2026-07-31)


### Bug Fixes

* **tui:** validate dropped file attachments ([#677](https://github.com/matthewyjiang/rho/issues/677)) ([1c62a3d](https://github.com/matthewyjiang/rho/commit/1c62a3d638328bc829350f1cf32649fd58f7abcb))

## [0.11.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.10.1...rho-agent-tools-v0.11.0) (2026-07-31)


### Features

* **documents:** add bounded document extraction and attachments ([#669](https://github.com/matthewyjiang/rho/issues/669)) ([d1ec3cd](https://github.com/matthewyjiang/rho/commit/d1ec3cd5d8f5683c7b8de0047070a6029bb1ec33))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.1 to 1.14.0

## [0.10.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.10.0...rho-agent-tools-v0.10.1) (2026-07-30)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.0 to 1.13.1

## [0.10.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.9.0...rho-agent-tools-v0.10.0) (2026-07-30)


### Features

* **tools:** restore simple edit_file ([#658](https://github.com/matthewyjiang/rho/issues/658)) ([ffac70f](https://github.com/matthewyjiang/rho/commit/ffac70f6d58d1532a4eedefbdc99463402adbf7b))
* **tui:** stream apply_patch diff cards ([#657](https://github.com/matthewyjiang/rho/issues/657)) ([e2c932e](https://github.com/matthewyjiang/rho/commit/e2c932e377f15ddfaab1e4700aa7d6f4e8ed0417))

## [0.9.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.8.0...rho-agent-tools-v0.9.0) (2026-07-30)


### Features

* **tools:** replace edit_file with codex-style apply_patch ([#653](https://github.com/matthewyjiang/rho/issues/653)) ([eef1555](https://github.com/matthewyjiang/rho/commit/eef155521c5492b9c7f34507e82b6b7f46b896a8))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.2 to 1.13.0

## [0.8.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.7.4...rho-agent-tools-v0.8.0) (2026-07-29)


### Features

* **sessions:** add workspace rewind checkpoints ([#638](https://github.com/matthewyjiang/rho/issues/638)) ([5a90b2d](https://github.com/matthewyjiang/rho/commit/5a90b2db5b1170f2701cbac1c0c7d056f9158754))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.1 to 1.12.2

## [0.7.4](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.7.3...rho-agent-tools-v0.7.4) (2026-07-28)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.12.0 to 1.12.1

## [0.7.3](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.7.2...rho-agent-tools-v0.7.3) (2026-07-28)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.11.0 to 1.12.0

## [0.7.2](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.7.1...rho-agent-tools-v0.7.2) (2026-07-28)


### Bug Fixes

* **agents:** clarify foreground agent batch behavior ([#606](https://github.com/matthewyjiang/rho/issues/606)) ([9574e48](https://github.com/matthewyjiang/rho/commit/9574e4836a3c6e14eb28bc5863b8d2abc334e140))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.3 to 1.11.0

## [0.7.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.7.0...rho-agent-tools-v0.7.1) (2026-07-27)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.2 to 1.10.3

## [0.7.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.6.2...rho-agent-tools-v0.7.0) (2026-07-27)


### Features

* **tui:** unify tool transcript cards as Call + Children ([#586](https://github.com/matthewyjiang/rho/issues/586)) ([ce52cdd](https://github.com/matthewyjiang/rho/commit/ce52cddb6dbf0ac1b2878b6f3bd468a87547f8fa))

## [0.6.2](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.6.1...rho-agent-tools-v0.6.2) (2026-07-27)


### Bug Fixes

* **sdk:** recover failed 1.17.1 release packaging ([#587](https://github.com/matthewyjiang/rho/issues/587)) ([224189e](https://github.com/matthewyjiang/rho/commit/224189e2d4fc2ec5f23cb88d80065d82c91ef40b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.1 to 1.10.2

## [0.6.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.6.0...rho-agent-tools-v0.6.1) (2026-07-26)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.10.0 to 1.10.1

## [0.6.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.6...rho-agent-tools-v0.6.0) (2026-07-26)


### Features

* **tools:** add in-process grep and glob workspace tools ([#554](https://github.com/matthewyjiang/rho/issues/554)) ([e422a99](https://github.com/matthewyjiang/rho/commit/e422a990332afff330b096d8960d4e0fa07a5838))

## [0.5.6](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.5...rho-agent-tools-v0.5.6) (2026-07-25)


### Bug Fixes

* **errors:** surface failures that were silently swallowed ([#546](https://github.com/matthewyjiang/rho/issues/546)) ([1d4eee3](https://github.com/matthewyjiang/rho/commit/1d4eee3ea2e45d459897198d48babbe3ded3bf19))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.9.0 to 1.10.0

## [0.5.5](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.4...rho-agent-tools-v0.5.5) (2026-07-24)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.8.0 to 1.9.0

## [0.5.4](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.3...rho-agent-tools-v0.5.4) (2026-07-23)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.2 to 1.8.0

## [0.5.3](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.2...rho-agent-tools-v0.5.3) (2026-07-22)


### Bug Fixes

* **tui:** paste image paths and fall back kitty under herdr ([#504](https://github.com/matthewyjiang/rho/issues/504)) ([c140bfe](https://github.com/matthewyjiang/rho/commit/c140bfe6994f4ffc42756075ec801eff6e63ce40))

## [0.5.2](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.1...rho-agent-tools-v0.5.2) (2026-07-22)


### Bug Fixes

* **tools:** scrub provider credential env vars from child processes ([#502](https://github.com/matthewyjiang/rho/issues/502)) ([6d66913](https://github.com/matthewyjiang/rho/commit/6d669135caa7aa160f8c81c109f0c99736b70e63))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.1 to 1.7.2

## [0.5.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.5.0...rho-agent-tools-v0.5.1) (2026-07-22)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.7.0 to 1.7.1

## [0.5.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.4.0...rho-agent-tools-v0.5.0) (2026-07-22)


### Features

* **auth:** add configurable credential storage ([#478](https://github.com/matthewyjiang/rho/issues/478)) ([e778eda](https://github.com/matthewyjiang/rho/commit/e778edab71ec7e3c2f21137760f53bd0b8089469))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.6.0 to 1.7.0

## [0.4.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.3.3...rho-agent-tools-v0.4.0) (2026-07-21)


### Features

* **sdk:** execute independent tool calls concurrently ([#459](https://github.com/matthewyjiang/rho/issues/459)) ([0bb5a83](https://github.com/matthewyjiang/rho/commit/0bb5a830adc191d09ab40726577483c72cecf74f))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.5.0 to 1.6.0

## [0.3.3](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.3.2...rho-agent-tools-v0.3.3) (2026-07-20)


### Bug Fixes

* **tools:** bound the timeout drain so escaped processes cannot stall bash ([#342](https://github.com/matthewyjiang/rho/issues/342)) ([414850f](https://github.com/matthewyjiang/rho/commit/414850fd374315f85691323d6bcf5615880da0d2))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.4.0 to 1.5.0

## [0.3.2](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.3.1...rho-agent-tools-v0.3.2) (2026-07-20)


### Bug Fixes

* **release:** guard publishable crate version bumps ([#424](https://github.com/matthewyjiang/rho/issues/424)) ([4b39b58](https://github.com/matthewyjiang/rho/commit/4b39b58cb09a2815be4d5350c2b0e0a831a426fe))

## [0.3.1](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.3.0...rho-agent-tools-v0.3.1) (2026-07-20)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.3.0 to 1.4.0

## [0.3.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.2.0...rho-agent-tools-v0.3.0) (2026-07-18)


### Features

* **tui:** render read file image previews ([#393](https://github.com/matthewyjiang/rho/issues/393)) ([52165ec](https://github.com/matthewyjiang/rho/commit/52165eccb9429cbfe80c6ec1390aa5e97be19df8))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.1.0 to 1.3.0

## [0.2.0](https://github.com/matthewyjiang/rho/compare/rho-agent-tools-v0.1.0...rho-agent-tools-v0.2.0) (2026-07-18)


### Features

* readmes for extracted library crates ([#388](https://github.com/matthewyjiang/rho/issues/388)) ([92c234d](https://github.com/matthewyjiang/rho/commit/92c234d6ef15ff85f7b68cb31ebdb479cb81f022))
