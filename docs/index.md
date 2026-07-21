---
layout: home

hero:
  name: Rho
  text: A lightweight agent harness inspired by Pi
  tagline: Built in Rust to stay fast and memory-efficient.
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started
    - theme: alt
      text: Rust SDK
      link: /sdk/
    - theme: alt
      text: Interactive TUI
      link: /interactive-tui

features:
  - title: Terminal-native
    details: Run rho to open a fullscreen terminal UI with streaming assistant output, reasoning deltas, compact tool blocks, and app-owned transcript scrolling.
  - title: Automation-ready
    details: Use rho run for one-shot prompts in scripts, hooks, aliases, and CI jobs.
  - title: Embeddable SDK
    details: Build headless Rust agents with explicit providers, tools, sessions, streaming events, cancellation, snapshots, and default-deny capabilities.
  - title: Rust-first
    details: Rho is built in Rust instead of TypeScript to avoid the runtime overhead of a Node.js-based harness.
---

## Rho in action

[![Rho terminal UI showing a code inspection, Rust edit, and focused test run](./assets/rho-ui-demo.svg)](/interactive-tui)

## Concept docs

- [Getting started](/getting-started)
- [Installation](/installation)
- [Authentication and models](/authentication-and-models)
  - [OpenAI](/providers/openai)
  - [OpenAI (Codex OAuth)](/providers/openai-codex)
  - [Anthropic](/providers/anthropic)
  - [Google Gemini](/providers/google-gemini)
  - [GitHub Copilot](/providers/github-copilot)
  - [Ollama](/providers/ollama)
  - [OpenRouter](/providers/openrouter)
  - [Moonshot and Kimi Code](/providers/moonshot-kimi)
  - [xAI](/providers/xai)
- [Interactive TUI](/interactive-tui)
- [Inline shell](/inline-shell)
- [Automation and CLI](/automation-cli)
- [Configuration](/configuration)
- [Tools and workspace](/tools-workspace)
- [Sessions](/sessions)
- [Usage ledger](/usage-ledger)
- [Rust SDK](/sdk/)
  - [Installation and support](/sdk/installation)
  - [Concepts and ownership](/sdk/concepts)
  - [Providers](/sdk/providers)
  - [Tools and capabilities](/sdk/tools)
  - [Sessions and persistence](/sdk/sessions-and-persistence)
  - [Events and cancellation](/sdk/events-and-cancellation)
  - [Compatibility contracts](/sdk/compatibility)
  - [Security model](/sdk/security)
  - [Threat model](/sdk/threat-model)
  - [Redaction audit procedure](/sdk/redaction-audit)
  - [Upgrade to 1.0](/sdk/upgrade-to-1.0)
  - [Release candidates](/sdk/release-candidates)
- [Development](/development)
- [Changelog](/changelog)
