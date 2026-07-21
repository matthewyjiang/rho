# Changelog

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
