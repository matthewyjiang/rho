import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Rho',
  description: 'A lightweight agent harness inspired by Pi',
  base: '/rho/',
  cleanUrls: true,
  themeConfig: {
    nav: [
      { text: 'Guide', link: '/guide/' },
      { text: 'Interactive TUI', link: '/interactive-tui' }
    ],
    sidebar: [
      {
        text: 'Rho',
        items: [
          { text: 'Overview', link: '/' },
          { text: 'Guide', link: '/guide/' },
          { text: 'Interactive TUI', link: '/interactive-tui' }
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
