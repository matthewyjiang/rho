# Changelog

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
