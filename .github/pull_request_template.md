## Summary

<!-- What changed and why. -->

## Validation

<!-- Commands and results. Narrowest tests first. Delete the proof-plate line unless this diff touches Interactive TUI layout or chrome, tool cards, the statusline, version display, the docs demo fixture, or rho-pty-demo. -->

```text
```

- [ ] `bash scripts/check_docs_ui_demo.sh --check` (or `--write`, then commit the dark SVGs and the site light SVG)

## Test gate

<!-- Delete this section if the diff adds no tests. Rules: `.agents/skills/rho-test-selection/SKILL.md`. -->

- [ ] Each new test names one failure mode, has one owner layer, and is not already covered.
- [ ] Interactive TUI behavior is a PTY scenario. A new `crates/rho/src/tui` unit test is pure logic, or the exception below says why PTY is the wrong layer.
- [ ] One table-driven test per rule. Structured asserts. No copy locks, sleep synchronization, or known flakes.
- [ ] Weaker or duplicate nearby tests were removed or merged when practical.

| Failure mode | Owner layer | Why existing coverage is not enough |
| --- | --- | --- |
| | | |

PTY exception, if any:

```text
N/A
```

## Major release

<!-- Delete unless the maintainer approved a major release. `!` or a `BREAKING CHANGE:` footer anywhere in the squash message publishes a new major. Describe the break and the migration. Behavior changes that ship in a minor belong in the summary. -->

## Next-major debt

<!-- Delete if none. Rules: `.agents/skills/rho-next-major-debt/SKILL.md`. -->

- [ ] Each minor-only compromise has a `NEXT_MAJOR(<surface>): <cleanup>` marker. It names the preferred end state, helpers cover every arm, and host docs match the awkward shape.

| Marker | Preferred end state |
| --- | --- |
| | |

<!-- If an AI made this change, end with one line naming the model and harness. -->
