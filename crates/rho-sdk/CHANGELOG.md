# Changelog

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
