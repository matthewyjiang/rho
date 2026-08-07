<script setup lang="ts">
withDefaults(
  defineProps<{
    size?: 'sm' | 'md' | 'lg' | 'xl'
    title?: string
    /** When true, mark is purely visual; parent supplies the accessible name. */
    decorative?: boolean
  }>(),
  {
    size: 'md',
    title: 'Rho',
    decorative: false,
  },
)
</script>

<template>
  <span
    class="rho-wordmark"
    :class="`rho-wordmark--${size}`"
    :aria-label="decorative ? undefined : title"
    :aria-hidden="decorative ? true : undefined"
  >
    <span class="rho-wordmark__glyph" aria-hidden="true">ρ</span>
    <span class="rho-wordmark__text" aria-hidden="true">Rho</span>
  </span>
</template>

<style scoped>
.rho-wordmark {
  display: inline-flex;
  align-items: baseline;
  gap: 0.22em;
  color: currentColor;
  line-height: 1;
  font-family: var(--rho-font-display, var(--vp-font-family-base));
  font-weight: 700;
  letter-spacing: -0.03em;
  white-space: nowrap;
}

.rho-wordmark__glyph {
  /* Public Sans is Latin-only; system UI covers Greek ρ without pulling Inter. */
  font-family: ui-sans-serif, system-ui, 'Segoe UI', 'Helvetica Neue', Arial, sans-serif;
  color: var(--rho-accent, #e63228);
  font-weight: 700;
  letter-spacing: 0;
  /* Slight optical lift so the descender sits with the wordmark. */
  transform: translateY(0.02em);
}
.rho-wordmark__text {
  display: inline-block;
}

.rho-wordmark--sm {
  font-size: 1.05rem;
  letter-spacing: -0.02em;
}

.rho-wordmark--sm .rho-wordmark__glyph {
  font-size: 1.08em;
}

.rho-wordmark--md {
  font-size: 1.35rem;
}

.rho-wordmark--md .rho-wordmark__glyph {
  font-size: 1.1em;
}

.rho-wordmark--lg {
  font-size: clamp(2.4rem, 6vw, 4rem);
}

.rho-wordmark--lg .rho-wordmark__glyph {
  font-size: 1.05em;
}

.rho-wordmark--xl {
  font-size: clamp(2.75rem, 7vw, 5rem);
}

.rho-wordmark--xl .rho-wordmark__glyph {
  font-size: 1.05em;
}
</style>
