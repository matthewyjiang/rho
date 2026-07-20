# Changelog

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
