import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Rho',
  description: 'A lightweight agent harness inspired by Pi',
  base: '/rho/',
  cleanUrls: true,
  themeConfig: {
    nav: [
      { text: 'Getting started', link: '/getting-started' },
      { text: 'Interactive TUI', link: '/interactive-tui' },
      { text: 'Automation', link: '/automation-cli' },
      { text: 'Rust SDK', link: '/sdk/' },
      { text: 'App changelog', link: '/changelog' }
    ],
    sidebar: [
      {
        text: 'Rho',
        items: [
          { text: 'Overview', link: '/' },
          { text: 'Getting started', link: '/getting-started' },
          { text: 'Installation', link: '/installation' },
          { text: 'Authentication and models', link: '/authentication-and-models' },
          {
            text: 'Providers',
            collapsed: false,
            items: [
              { text: 'OpenAI', link: '/providers/openai' },
              { text: 'OpenAI (Codex OAuth)', link: '/providers/openai-codex' },
              { text: 'Anthropic', link: '/providers/anthropic' },
              { text: 'Google Gemini', link: '/providers/google-gemini' },
              { text: 'GitHub Copilot', link: '/providers/github-copilot' },
              { text: 'Ollama', link: '/providers/ollama' },
              { text: 'OpenRouter', link: '/providers/openrouter' },
              { text: 'Moonshot and Kimi Code', link: '/providers/moonshot-kimi' },
              { text: 'xAI', link: '/providers/xai' }
            ]
          },
          { text: 'Interactive TUI', link: '/interactive-tui' },
          { text: 'Automation and CLI', link: '/automation-cli' },
          { text: 'Configuration', link: '/configuration' },
          { text: 'Tools and workspace', link: '/tools-workspace' },
          { text: 'Skills', link: '/skills' },
          { text: 'Subagents', link: '/subagents' },
          { text: 'Sessions', link: '/sessions' },
          { text: 'Development', link: '/development' },
          { text: 'App changelog', link: '/changelog' }
        ]
      },
      {
        text: 'Rust SDK',
        items: [
          { text: 'Overview', link: '/sdk/' },
          { text: 'Installation and support', link: '/sdk/installation' },
          { text: 'Concepts and ownership', link: '/sdk/concepts' },
          { text: 'Providers', link: '/sdk/providers' },
          { text: 'Tools and capabilities', link: '/sdk/tools' },
          { text: 'Sessions and persistence', link: '/sdk/sessions-and-persistence' },
          { text: 'Events and cancellation', link: '/sdk/events-and-cancellation' },
          { text: 'Compatibility contracts', link: '/sdk/compatibility' },
          { text: 'Security model', link: '/sdk/security' },
          { text: 'Threat model', link: '/sdk/threat-model' },
          { text: 'Redaction audit', link: '/sdk/redaction-audit' },
          { text: 'Performance acceptance', link: '/sdk/performance' },
          { text: 'Upgrade to planned 1.0', link: '/sdk/upgrade-to-1.0' },
          { text: 'Draft 1.0 release notes', link: '/sdk/release-notes-1.0' },
          { text: 'Release candidates', link: '/sdk/release-candidates' },
          { text: 'SDK changelog', link: '/sdk/changelog' }
        ]
      }
    ],
    socialLinks: [
      { icon: 'github', link: 'https://github.com/matthewyjiang/rho' }
    ],
    search: {
      provider: 'local'
    }
  }
})
