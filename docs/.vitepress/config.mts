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
      { text: 'Changelog', link: '/changelog' }
    ],
    sidebar: [
      {
        text: 'Rho',
        items: [
          { text: 'Overview', link: '/' },
          { text: 'Getting started', link: '/getting-started' },
          { text: 'Installation', link: '/installation' },
          { text: 'Authentication and models', link: '/authentication-and-models' },
          { text: 'Interactive TUI', link: '/interactive-tui' },
          { text: 'Automation and CLI', link: '/automation-cli' },
          { text: 'Configuration', link: '/configuration' },
          { text: 'Tools and workspace', link: '/tools-workspace' },
          { text: 'Sessions', link: '/sessions' },
          { text: 'Development', link: '/development' },
          { text: 'Changelog', link: '/changelog' }
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
