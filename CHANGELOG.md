# Changelog

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
