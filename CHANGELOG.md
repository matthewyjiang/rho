# Changelog

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
