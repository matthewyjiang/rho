import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { withMermaid } from 'vitepress-plugin-mermaid'
import type { DefaultTheme } from 'vitepress'
import type { Plugin as VitePlugin } from 'vite'

const configDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(configDir, '../..')
const installScripts = ['install.sh', 'install.ps1'] as const

/** Serve scripts/install.* at the docs site root (for example /rho/install.sh). */
function installScriptsPlugin(): VitePlugin {
  const scriptPath = (name: (typeof installScripts)[number]) =>
    path.join(repoRoot, 'scripts', name)

  const matchScript = (url: string | undefined) => {
    const pathname = (url ?? '').split('?')[0] ?? ''
    return installScripts.find(
      (name) => pathname === `/${name}` || pathname.endsWith(`/${name}`),
    )
  }

  return {
    name: 'rho-install-scripts',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const name = matchScript(req.url)
        if (!name) {
          next()
          return
        }

        res.statusCode = 200
        res.setHeader('Content-Type', 'text/plain; charset=utf-8')
        res.setHeader('Cache-Control', 'no-cache')
        res.end(fs.readFileSync(scriptPath(name)))
      })
    },
    writeBundle(options) {
      if (!options.dir) {
        return
      }

      for (const name of installScripts) {
        fs.copyFileSync(scriptPath(name), path.join(options.dir, name))
      }
    },
  }
}

const providerItems: DefaultTheme.SidebarItem[] = [
  { text: 'OpenAI', link: '/providers/openai' },
  { text: 'OpenAI (Codex OAuth)', link: '/providers/openai-codex' },
  { text: 'Anthropic', link: '/providers/anthropic' },
  { text: 'Google Gemini', link: '/providers/google-gemini' },
  { text: 'GitHub Copilot', link: '/providers/github-copilot' },
  { text: 'Ollama', link: '/providers/ollama' },
  { text: 'Ollama Cloud', link: '/providers/ollama-cloud' },
  { text: 'OpenRouter', link: '/providers/openrouter' },
  { text: 'Poolside', link: '/providers/poolside' },
  { text: 'Moonshot and Kimi Code', link: '/providers/moonshot-kimi' },
  { text: 'Qwen Token Plan', link: '/providers/qwen-token-plan' },
  { text: 'Meta Model API', link: '/providers/meta' },
  { text: 'MiniMax', link: '/providers/minimax' },
  { text: 'OpenCode Go', link: '/providers/opencode-go' },
  { text: 'xAI', link: '/providers/xai' },
]

const appSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: 'Start',
    items: [
      { text: 'Overview', link: '/' },
      { text: 'Getting started', link: '/getting-started' },
      { text: 'Installation', link: '/installation' },
      { text: 'Authentication and models', link: '/authentication-and-models' },
      {
        text: 'Providers',
        collapsed: true,
        items: [
          { text: 'Provider index', link: '/providers/' },
          ...providerItems,
        ],
      },
    ],
  },
  {
    text: 'Use Rho',
    items: [
      {
        text: 'Interactive TUI',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/interactive-tui' },
          { text: 'Attachments', link: '/interactive-tui/attachments' },
          { text: 'Transcript display', link: '/interactive-tui/transcript' },
          { text: 'Theme', link: '/interactive-tui/theme' },
          { text: 'Mermaid diagrams', link: '/interactive-tui/mermaid' },
          { text: 'Math rendering', link: '/interactive-tui/math' },
        ],
      },
      { text: 'Inline shell', link: '/inline-shell' },
      { text: 'Automation and CLI', link: '/automation-cli' },
      {
        text: 'Workflows',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/workflows' },
          { text: 'Authoring', link: '/workflows/authoring' },
          { text: 'Runtime', link: '/workflows/runtime' },
        ],
      },
      { text: 'Sessions', link: '/sessions' },
      {
        text: 'Integrations',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/integrations' },
          { text: 'Herdr', link: '/integrations/herdr' },
          { text: 'RTK', link: '/integrations/rtk' },
          { text: 'Model Context Protocol', link: '/integrations/mcp' },
          { text: 'Agent Client Protocol', link: '/integrations/acp' },
          { text: 'Agent Plugins', link: '/integrations/plugins' },
        ],
      },
    ],
  },
  {
    text: 'Customize',
    items: [
      {
        text: 'Configuration',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/configuration' },
          { text: 'Advisor mode', link: '/configuration/advisor-mode' },
          { text: 'Full example', link: '/configuration/full-example' },
        ],
      },
      {
        text: 'Tools and workspace',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/tools-workspace' },
          { text: 'Edit format', link: '/tools-workspace/edit-format' },
          { text: 'Search tools', link: '/tools-workspace/search' },
          { text: 'Documents and images', link: '/tools-workspace/documents-and-images' },
          { text: 'Web access', link: '/tools-workspace/web-access' },
          { text: 'Background processes', link: '/tools-workspace/background-processes' },
        ],
      },
      { text: 'Skills', link: '/skills' },
      {
        text: 'Hooks',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/hooks' },
          { text: 'Protocol', link: '/hooks/protocol' },
        ],
      },
      {
        text: 'Subagents',
        collapsed: true,
        items: [
          { text: 'Overview', link: '/subagents' },
          { text: 'Definition schema', link: '/subagents/definition-schema' },
          { text: 'Binding and security', link: '/subagents/binding-and-security' },
          { text: 'Claude Code runtime', link: '/subagents/claude-cli' },
          { text: 'Attachment and artifacts', link: '/subagents/attachment-and-artifacts' },
        ],
      },
      { text: 'Usage ledger', link: '/usage-ledger' },
    ],
  },
  {
    text: 'Project',
    items: [
      { text: 'Development', link: '/development' },
      { text: 'App changelog', link: '/changelog' },
      { text: 'Rust SDK', link: '/sdk/' },
    ],
  },
]

const sdkSidebar: DefaultTheme.SidebarItem[] = [
  {
    text: 'SDK guide',
    items: [
      { text: 'Overview', link: '/sdk/' },
      { text: 'Installation and support', link: '/sdk/installation' },
      { text: 'Concepts and ownership', link: '/sdk/concepts' },
      { text: 'Providers', link: '/sdk/providers' },
      { text: 'Tools and capabilities', link: '/sdk/tools' },
      { text: 'Hooks', link: '/sdk/hooks' },
      { text: 'Sessions and persistence', link: '/sdk/sessions-and-persistence' },
      { text: 'Events and cancellation', link: '/sdk/events-and-cancellation' },
    ],
  },
  {
    text: 'Security',
    items: [
      { text: 'Security model', link: '/sdk/security' },
      { text: 'Threat model', link: '/sdk/threat-model' },
      { text: 'Redaction audit', link: '/sdk/redaction-audit' },
    ],
  },
  {
    text: 'Reference and history',
    collapsed: true,
    items: [
      { text: 'Compatibility contracts', link: '/sdk/compatibility' },
      { text: 'Performance acceptance', link: '/sdk/performance' },
      { text: 'SDK changelog', link: '/sdk/changelog' },
      { text: 'Upgrade to 1.0 (historical)', link: '/sdk/upgrade-to-1.0' },
      { text: '1.0 release notes (historical)', link: '/sdk/release-notes-1.0' },
      { text: 'Release candidates (historical)', link: '/sdk/release-candidates' },
    ],
  },
  {
    text: 'Rho app docs',
    items: [
      { text: 'Getting started', link: '/getting-started' },
      { text: 'Interactive TUI', link: '/interactive-tui' },
      { text: 'Automation and CLI', link: '/automation-cli' },
    ],
  },
]

export default withMermaid({
  title: 'Rho',
  description: 'A fast Rust agent harness with a small footprint and opinionated defaults.',
  base: '/rho/',
  cleanUrls: true,
  lastUpdated: true,
  appearance: true,
  mermaid: {
    securityLevel: 'strict',
    theme: 'neutral',
  },
  mermaidPlugin: {
    class: 'rho-mermaid',
  },
  vite: {
    plugins: [installScriptsPlugin()],
  },
  themeConfig: {
    nav: [
      { text: 'Getting started', link: '/getting-started' },
      {
        text: 'Guide',
        items: [
          { text: 'Interactive TUI', link: '/interactive-tui' },
          { text: 'Automation and CLI', link: '/automation-cli' },
          { text: 'Workflows', link: '/workflows' },
          { text: 'Configuration', link: '/configuration' },
          { text: 'Tools and workspace', link: '/tools-workspace' },
          { text: 'Subagents', link: '/subagents' },
          { text: 'Sessions', link: '/sessions' },
          { text: 'Integrations', link: '/integrations' },
        ],
      },
      {
        text: 'Providers',
        items: [
          { text: 'Authentication and models', link: '/authentication-and-models' },
          { text: 'Provider index', link: '/providers/' },
          ...providerItems,
        ],
      },
      { text: 'Rust SDK', link: '/sdk/' },
      {
        text: 'Changelog',
        items: [
          { text: 'App changelog', link: '/changelog' },
          { text: 'SDK changelog', link: '/sdk/changelog' },
        ],
      },
    ],
    sidebar: {
      '/sdk/': sdkSidebar,
      '/': appSidebar,
    },
    socialLinks: [
      { icon: 'github', link: 'https://github.com/matthewyjiang/rho' },
    ],
    search: {
      provider: 'local',
    },
    editLink: {
      pattern: 'https://github.com/matthewyjiang/rho/edit/main/docs/:path',
      text: 'Edit this page on GitHub',
    },
    outline: {
      level: [2, 3],
    },
  },
})
