# AGENTS.md

## Conventional commits

Use Conventional Commits for all commit messages and PR titles.

Format:

```text
<type>(<scope>): <description>
```

- `type` must be one of: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, or `revert`.
- `scope` is optional, but preferred when it makes the affected area clear.
- `description` must be concise, imperative, and lowercase unless it contains a proper noun.
- Do not end the description with a period.

Examples:

```text
feat(auth): add token refresh
fix(api): handle empty responses
docs: update setup instructions
chore: bump dependencies
```

## Commits

- Write every commit message using Conventional Commits.
- Keep commits focused on one logical change.
- Use a body when the change needs context, migration notes, or tradeoffs.
- For breaking changes, add `!` after the type or scope and include a `BREAKING CHANGE:` footer.

Example breaking change:

```text
feat(config)!: require explicit config path

BREAKING CHANGE: the default config discovery behavior was removed.
```

## Pull requests

- Use a Conventional Commit style PR title, because it may become the squash commit title.
- Prefer the most user-visible type for the PR title, usually `feat`, `fix`, `docs`, or `refactor`.
- Include a clear summary of what changed and why.
- List tests or validation performed.
- Call out breaking changes with a `BREAKING CHANGE:` section.

PR title examples:

```text
feat(cli): add init command
fix(parser): preserve escaped whitespace
refactor(storage): simplify cache writes
```
