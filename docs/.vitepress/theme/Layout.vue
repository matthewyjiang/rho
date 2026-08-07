<script setup lang="ts">
import DefaultTheme from 'vitepress/theme'
import RhoWordmark from './components/RhoWordmark.vue'

const { Layout } = DefaultTheme
</script>

<template>
  <!--
    THESIS: Print-specimen clarity over dense reading columns; native binary energy without AI-marketing theater.
    OWN-WORLD: Newsprint ground, rich black ink, one synthetic red accent; Public Sans for UI/body, Source Code Pro for code; hairline rules and ink plates.
    STORY: Visitor knows Rho is a small Rust harness, sees the TUI proof, and picks install, TUI, automation, or SDK path.
    FIRST VIEWPORT: Large Rho wordmark, job line, install specimen, dual actions, TUI proof plate.
    FORM: Emigre Bitmap Specimen (challenger), seed 759ff781.
    FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, and DESIGN.md
  -->
  <Layout>
    <template #layout-top>
      <span
        id="rho-direction-contract"
        hidden
        data-direction="emigre-bitmap-specimen"
        data-seed="759ff781"
      >
        THESIS: Print-specimen clarity over dense reading columns; native binary energy without AI-marketing theater.
        OWN-WORLD: Newsprint ground, rich black ink, one synthetic red accent; Public Sans for UI/body, Source Code Pro for code; hairline rules and ink plates.
        STORY: Visitor knows Rho is a small Rust harness, sees the TUI proof, and picks install, TUI, automation, or SDK path.
        FIRST VIEWPORT: Large Rho wordmark, job line, install specimen, dual actions, TUI proof plate.
        FORM: Emigre Bitmap Specimen (challenger), seed 759ff781.
        FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, and DESIGN.md
      </span>
    </template>

    <template #nav-bar-title-before>
      <!-- Decorative mark; visible title text remains the accessible name. -->
      <span class="rho-nav-mark" aria-hidden="true">
        <RhoWordmark size="sm" decorative />
      </span>
    </template>
  </Layout>
</template>

<style>
.rho-nav-mark {
  display: inline-flex;
  color: var(--vp-c-text-1);
}

.VPNavBarTitle .title {
  display: inline-flex;
  align-items: center;
  gap: 0;
  border: 0 !important;
}

/* Wordmark replaces the text title visually; keep text for screen readers / fallback. */
.VPNavBarTitle .title > span:not(.rho-nav-mark) {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}

/* Full-bleed custom home on the page layout */
.VPPage:has(.rho-home),
.VPDoc:has(.rho-home) {
  padding: 0 !important;
}

.VPPage:has(.rho-home) {
  max-width: none !important;
  margin: 0 !important;
}

.VPDoc:has(.rho-home) .container {
  max-width: none !important;
  margin: 0 !important;
}

.VPDoc:has(.rho-home) .content,
.VPDoc:has(.rho-home) .content-container,
.VPDoc:has(.rho-home) .main {
  max-width: none !important;
  padding: 0 !important;
  margin: 0 !important;
}

.VPDoc:has(.rho-home) .vp-doc {
  margin: 0 !important;
  padding: 0 !important;
}

.VPDoc:has(.rho-home) .aside {
  display: none !important;
}
</style>
