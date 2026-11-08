# T3 Code

Parent: [Agent Client Protocol](/integrations/acp).

[T3 Code](https://github.com/pingdotgg/t3code) runs Rho over ACP. In T3 Code, add Rho as an ACP agent with the command `rho` and the arguments `acp --t3code`:

```bash
rho acp --t3code
```

`--t3code` is accepted, but its T3 integration is not yet available with the daemon ([#1670](https://github.com/matthewyjiang/rho/issues/1670)): Rho does not run subagents as T3 child threads, and background results do not wake an idle thread. Rho runs its subagents itself, in the daemon, and background work holds the prompt open, as described in [Background work](/integrations/acp#background-work).

Rho negotiates ACP v2 when T3 Code requests it; v1 remains available when a host requests v1. Use an up-to-date T3 Code build: versions predating the October 30, 2026 prompt-echo fix can show the injected `<t3_code_instructions>` as a duplicate user message on v2. Upgrade T3 Code rather than downgrading Rho. Notices use native notice rows instead of assistant reply text.

## Sign in

T3 Code's sign-in terminal appends `--login <auth-profile>` to the configured command, for example `rho acp --t3code --login codex`, which signs in through the daemon instead of serving ACP. Sign-in works even when the configured provider has no credentials yet. When a prompt finds credentials missing, unreadable, or refused, T3 Code reports that the agent needs terminal authentication. See [Sign in](/integrations/acp#sign-in).

## How T3 Code shows Rho

T3 Code draws some ACP features differently from other hosts:

| Feature | In T3 Code |
| --- | --- |
| Plan approval | T3's usual approve, approve for session, and decline buttons. They pick `supervised`, `allow_edits`, and keep planning. `bypass` and `auto` cannot be chosen there. A thread in full access approves without showing the plan. A sandboxed thread may reject every time; switch the session's mode by hand. |
| Questions | Every question needs an answer, typed as text. Name a choice by its label or value, and separate several with commas or the separator the form names. Free text on a question that only accepts its listed choices counts as declining. Exact rules: [T3 Code forms](/integrations/acp/reference#t3-code-forms). |
| Checklist | Current T3 builds replace the checklist when Rho clears its `todo` list. Builds predating the October 30, 2026 empty-plan fix can retain the previous list. |
| Manual compaction | T3 intercepts bare `/compact` and currently rejects it for generic ACP agents before Rho receives it. Rho's automatic compaction is unaffected. |
| Interrupting | When you change settings and send a message mid-prompt, the message you send runs with the new settings. |
| Background results | Result cards show the plain output, without markdown fences. |
| Skills | `$<name>` skills appear in T3's `$` menu. |
| Titles | T3 records Rho's generated session title. |

The protocol behind each of these is in the [ACP host reference](/integrations/acp/reference).
