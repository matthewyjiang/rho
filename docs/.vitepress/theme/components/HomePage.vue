<script setup lang="ts">
import { withBase } from 'vitepress'
import RhoWordmark from './RhoWordmark.vue'

/** One-line install command shown in a native VitePress code plate. */
const installCommand =
  'curl -fsSL https://matthewyjiang.github.io/rho/install.sh | sh'

const paths = [
  {
    title: 'Install and first run',
    detail: 'Getting started → install → auth → model',
    link: '/getting-started',
  },
  {
    title: 'Terminal UI',
    detail: 'Interactive TUI, inline shell, sessions',
    link: '/interactive-tui',
  },
  {
    title: 'Automation and CI',
    detail: 'rho run, workflows, scripting',
    link: '/automation-cli',
  },
  {
    title: 'Configure and extend',
    detail: 'Config, tools, skills, hooks, subagents, plugins',
    link: '/configuration',
  },
  {
    title: 'Embed in Rust',
    detail: 'rho-sdk providers, tools, sessions, events',
    link: '/sdk/',
  },
  {
    title: 'Contribute',
    detail: 'Development guide and changelogs',
    link: '/development',
  },
] as const
</script>

<template>
  <div class="rho-home">
    <section class="rho-home__hero">
      <div class="rho-home__hero-grid">
        <div class="rho-home__hero-copy">
          <h1 class="rho-home__title">
            <RhoWordmark size="xl" />
          </h1>
          <p class="rho-home__text">A lightweight agent harness inspired by Pi. Built in Rust.</p>
          <p class="rho-home__pitch">
            Orchestrate agents across providers, including
            <a :href="withBase('/subagents/claude-cli')">Claude Code agents on your Claude subscription</a>.
            <a class="rho-home__pitch-more" :href="withBase('/subagents')">Set up agents</a>
          </p>

          <!--
            Match VitePress markdown fence markup so useCopyCode handles the button.
            .vp-doc wrapper pulls in default plate + copy styles (home is layout: page).
          -->
          <div class="rho-home__install vp-doc">
            <div class="language-sh vp-adaptive-theme">
              <button title="Copy Code" class="copy" type="button"></button>
              <span class="lang">sh</span>
              <pre class="shiki vp-code" tabindex="0"><code><span class="line"><span>{{ installCommand }}</span></span></code></pre>
            </div>
          </div>

          <div class="rho-home__actions">
            <a
              class="rho-home__action rho-home__action--brand"
              :href="withBase('/getting-started')"
            >
              Get started
            </a>
            <a
              class="rho-home__action rho-home__action--alt"
              :href="withBase('/sdk/')"
            >
              Rust SDK
            </a>
          </div>
        </div>

        <figure class="rho-home__proof">
          <a :href="withBase('/interactive-tui')" class="rho-home__proof-link">
            <img
              class="rho-home__proof-img"
              :src="withBase('/assets/rho-ui-demo.svg')"
              width="1008"
              height="848"
              alt="Rho terminal UI showing a request-ID middleware edit with read, edit, and test tool cards"
            />
          </a>
          <figcaption class="rho-home__proof-cap">
            Interactive TUI
          </figcaption>
        </figure>
      </div>
    </section>

    <section class="rho-home__band" aria-labelledby="rho-paths-heading">
      <div class="rho-home__band-head">
        <h2 id="rho-paths-heading" class="rho-home__h2">Guides</h2>
      </div>
      <ul class="rho-home__paths">
        <li v-for="path in paths" :key="path.link">
          <a class="rho-home__path" :href="withBase(path.link)">
            <span class="rho-home__path-title">{{ path.title }}</span>
            <span class="rho-home__path-detail">{{ path.detail }}</span>
            <span class="rho-home__path-arrow" aria-hidden="true">→</span>
          </a>
        </li>
      </ul>
    </section>

    <section class="rho-home__band rho-home__band--close">
      <p class="rho-home__lede">
        Coding tools, workflows, RTK, Herdr, and MCP ship in the binary. Plugins are optional.
        <a :href="withBase('/providers/')">Providers</a>
        and
        <a :href="withBase('/authentication-and-models')">authentication</a>
        cover model setup.
        Source on
        <a href="https://github.com/matthewyjiang/rho">GitHub</a>.
      </p>
    </section>
  </div>
</template>

<style scoped>
.rho-home {
  --rho-home-max: 1120px;
  color: var(--vp-c-text-1);
  background: var(--vp-c-bg);
}

.rho-home a {
  color: inherit;
  text-decoration-thickness: 1px;
  text-underline-offset: 0.18em;
}

.rho-home a:hover {
  color: var(--rho-accent);
}

.rho-home__hero,
.rho-home__band {
  max-width: var(--rho-home-max);
  margin: 0 auto;
  padding: 3.25rem 1.5rem 2.25rem;
}

.rho-home__hero {
  padding-top: 4rem;
  border-bottom: 1px solid var(--rho-rule);
}

.rho-home__hero-grid {
  display: grid;
  gap: 2.25rem;
  align-items: end;
}

@media (min-width: 900px) {
  .rho-home__hero-grid {
    grid-template-columns: minmax(16rem, 0.9fr) minmax(0, 1.1fr);
    gap: 2.5rem;
    align-items: start;
  }
}

.rho-home__title {
  margin: 0 0 1rem;
  line-height: 1;
}

.rho-home__text {
  margin: 0;
  /* Wider than a tight poster measure so mid breakpoints keep phrase boundaries. */
  max-width: 16em;
  font-size: clamp(1.45rem, 2.6vw, 2.05rem);
  font-weight: 700;
  line-height: 1.18;
  letter-spacing: -0.025em;
  text-wrap: balance;
}

/* Supporting claim under the job line; body weight so the lede stays primary. */
.rho-home__pitch {
  margin: 0.85rem 0 0;
  max-width: 36em;
  font-size: 1.02rem;
  line-height: 1.5;
  color: var(--vp-c-text-2);
  text-wrap: pretty;
}

.rho-home__pitch a {
  font-weight: 600;
  text-decoration: underline;
  text-underline-offset: 0.18em;
}

.rho-home__pitch-more {
  white-space: nowrap;
}

.rho-home__pitch-more::after {
  content: ' →';
}

.rho-home__install {
  margin-top: 1.6rem;
  max-width: 100%;
}

/* Compact home fence: same light/dark plate as doc code blocks. */
.rho-home__install.vp-doc div[class*='language-'] {
  margin: 0 !important;
  border: 1px solid var(--rho-rule) !important;
  border-radius: 0 !important;
  box-shadow: none !important;
  background: var(--vp-code-block-bg) !important;
}

.rho-home__install.vp-doc div[class*='language-'] pre {
  margin: 0;
  padding: 0.95rem 3.25rem 0.95rem 1rem !important;
  background: transparent !important;
  overflow-x: hidden !important;
  white-space: pre-wrap;
}

.rho-home__install.vp-doc div[class*='language-'] code {
  display: block;
  width: 100% !important;
  min-width: 0 !important;
  padding: 0 !important;
  font-size: clamp(0.72rem, 1.65vw, 0.86rem);
  line-height: 1.5;
  white-space: pre-wrap !important;
  overflow-wrap: anywhere;
  word-break: break-word;
  color: var(--vp-code-block-color) !important;
  background: transparent !important;
}

.rho-home__install.vp-doc div[class*='language-'] code .line,
.rho-home__install.vp-doc div[class*='language-'] code .line > span {
  color: inherit !important;
}

.rho-home__install.vp-doc [class*='language-'] > button.copy {
  opacity: 1;
  top: 0.65rem;
  right: 0.65rem;
  width: 2.25rem;
  height: 2.25rem;
  border-radius: 0 !important;
}

.rho-home__install.vp-doc [class*='language-'] > span.lang {
  top: 0.45rem;
  right: 3.1rem;
  color: var(--vp-code-lang-color);
}

.rho-home__actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.75rem;
  margin-top: 1.35rem;
}

.rho-home__action {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 2.75rem;
  padding: 0.65rem 1.15rem;
  border: 1px solid var(--vp-c-text-1);
  font-family: var(--rho-font-display);
  font-size: 0.95rem;
  font-weight: 600;
  letter-spacing: -0.015em;
  text-decoration: none !important;
  transition: background-color 120ms ease-out, color 120ms ease-out, border-color 120ms ease-out;
}

.rho-home__action--brand {
  background: var(--rho-accent);
  border-color: var(--rho-accent);
  color: #fff !important;
}

.rho-home__action--brand:hover {
  background: var(--rho-accent-deep);
  border-color: var(--rho-accent-deep);
  color: #fff !important;
}

.rho-home__action--alt {
  background: transparent;
  color: var(--vp-c-text-1) !important;
}

.rho-home__action--alt:hover {
  background: var(--vp-c-text-1);
  color: var(--vp-c-bg) !important;
}

.rho-home__proof {
  margin: 0;
}

.rho-home__proof-link {
  display: block;
  border: 1px solid var(--vp-c-text-1);
  background: #0d1117;
  text-decoration: none !important;
}

.rho-home__proof-img {
  display: block;
  width: 100%;
  height: auto;
  vertical-align: middle;
}

.rho-home__proof-cap {
  margin-top: 0.55rem;
  font-family: var(--vp-font-family-mono);
  font-size: 0.78rem;
  color: var(--vp-c-text-2);
}

.rho-home__band {
  border-bottom: 1px solid var(--rho-rule);
}

.rho-home__band-head {
  margin-bottom: 1.25rem;
}

.rho-home__h2 {
  margin: 0 0 0.5rem;
  font-family: var(--rho-font-display);
  font-size: clamp(1.25rem, 2vw, 1.5rem);
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1.25;
}

.rho-home__lede {
  margin: 0;
  max-width: 62ch;
  font-size: 1.02rem;
  line-height: 1.55;
  color: var(--vp-c-text-2);
}

.rho-home__lede a {
  font-weight: 600;
  text-decoration: underline;
}

.rho-home__paths {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: 0;
  border-top: 1px solid var(--rho-rule);
}

.rho-home__path {
  display: grid;
  grid-template-columns: 1fr auto;
  grid-template-rows: auto auto;
  column-gap: 0.75rem;
  row-gap: 0.2rem;
  padding: 1rem 0;
  border-bottom: 1px solid var(--rho-rule);
  text-decoration: none !important;
  transition: color 120ms ease-out;
}

.rho-home__path:hover {
  color: inherit;
}

.rho-home__path:hover .rho-home__path-title {
  color: var(--rho-accent);
}

.rho-home__path-title {
  grid-column: 1;
  font-weight: 700;
  letter-spacing: -0.02em;
}

.rho-home__path-detail {
  grid-column: 1;
  font-size: 0.92rem;
  color: var(--vp-c-text-2);
  line-height: 1.4;
}

.rho-home__path-arrow {
  grid-column: 2;
  grid-row: 1 / span 2;
  align-self: center;
  font-family: var(--rho-font-display);
  color: var(--rho-accent);
}

@media (min-width: 720px) {
  .rho-home__paths {
    grid-template-columns: 1fr 1fr;
    column-gap: 2.5rem;
  }
}

.rho-home__band--close {
  border-bottom: 0;
  padding-bottom: 4rem;
}
</style>
