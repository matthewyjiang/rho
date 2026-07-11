# Changelog

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
