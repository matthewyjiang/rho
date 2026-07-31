---
name: rho-workflow-authoring
description: Write, validate, plan, run, inspect, cancel, and resume deterministic Rho workflows in Starlark. Use when a task needs a fixed multi-step graph, parallel nodes, typed outputs, or durable resume.
---

# Author Rho workflows

Use this skill to write a `.star` file and operate it with `rho workflow` or the
`workflow` tool. CLI and tool help are the source of truth. This skill helps you
author a workflow, but it is not a security boundary.

## Safe authoring flow

1. Put the workflow under the current workspace or project root.
2. Declare each external value as an explicit input.
3. Build a finite directed acyclic graph in one `build(inputs)` call.
4. Use typed output schemas before a later node reads an output.
5. Use `command` for direct argv. Use `shell` only when shell syntax is needed.
6. Declare `access = "read_only"` only for a Rho agent whose frozen tools
   enforce read-only work.
7. Validate the source.
8. Create and inspect a frozen plan.
9. Confirm and run that plan ID.
10. Inspect status and artifact references. Cancel or resume by run ID.

Starlark runs during `validate` and `plan`. It does not run during `run` or
`resume`. Run and resume use the frozen graph only. Plan approval accepts one
graph digest for one run. It does not grant any node capability. Each node still
uses current policy, hooks, and approval rules.

Workflow inputs are persisted and shown in the plan. They are not a secret
store. Keep credentials in Rho credential stores or provider authentication.

## Complete example

```starlark
def build(inputs):
    inspect = agent(
        name = "inspect",
        agent = "reviewer",
        prompt = template([
            "Inspect ",
            inputs["target"],
            ". Return the required JSON value only.",
        ]),
        access = "read_only",
        output = record({
            "decision": enum(["approve", "revise"]),
            "summary": string(),
            "note": optional(string()),
        }),
        timeout_seconds = 1800,
        max_output_bytes = 12000,
    )

    test = command(
        name = "test",
        argv = ["cargo", "test", "-p", inputs["package"]],
        cwd = ".",
        needs = ["inspect"],
        when = equals(output("inspect", ["decision"]), "approve"),
        timeout_seconds = 1800,
        max_output_bytes = 12000,
        allow_failure = True,
        output = stdout_json(record({"passed": bool()})),
    )

    report = agent(
        name = "report",
        agent = "writer",
        needs = ["inspect", "test"],
        when = is_one_of(status("test"), ["success", "failure"]),
        prompt = template([
            "Write the report. Inspection summary: ",
            output("inspect", ["summary"]),
        ]),
        access = "mutating",
        timeout_seconds = 1800,
        max_output_bytes = 12000,
    )

    return workflow(name = "review", nodes = [inspect, test, report])

WORKFLOW = define(
    inputs = {
        "target": input.string(default = "."),
        "package": input.string(),
    },
    build = build,
)
```

Supply input values as JSON:

```bash
rho workflow validate .rho/workflows/review.star \
  --input 'package="rho-coding-agent"'
rho workflow plan .rho/workflows/review.star \
  --input 'package="rho-coding-agent"' --output json
```

With the model-facing tool, pass an input object:

```json
{"action":"validate","file":".rho/workflows/review.star","inputs":{"package":"rho-coding-agent"}}
```

## Entry and modules

The entry module must export exactly one `WORKFLOW` value from `define`. Rho
validates inputs before it calls `build`, and calls `build` once. Loaded modules
may define macros and helper functions.

Load a module by a root label:

```starlark
load("//.rho/workflows/common.star", "review_nodes")
```

All modules must be `.star` files below the module root. Labels cannot contain
an absolute path, `..`, an empty path part, a platform prefix, or a symlink.
Rho loads each module once and rejects import cycles. Starlark has no process,
filesystem, network, environment, clock, or random API.

Accepted values are `None`, booleans, signed 64-bit integers, strings, lists,
and dictionaries with string keys. Floats and sets are not supported.

## Inputs

Use these input constructors:

```starlark
input.string(default = "optional default")
input.integer(default = 3)
input.bool(default = False)
input.enum(["fast", "full"], default = "fast")
```

An input with no default is required. Rho rejects missing inputs, unknown
inputs, wrong types, and values outside an enum.

## Nodes

Every node has a portable unique `name`. Every node must set positive
`timeout_seconds` and `max_output_bytes` limits. A node can also set:

- `needs`: node names that must reach a terminal state first
- `when`: a typed condition
- `allow_failure`: whether failure can still permit workflow success
- `output`: a typed output contract

### Agent node

```starlark
agent(
    name = "review",
    agent = "reviewer",
    prompt = template(["Review ", inputs["target"]]),
    access = "read_only",
    needs = [],
    when = None,
    allow_failure = False,
    output = record({"summary": string()}),
    timeout_seconds = 1800,
    max_output_bytes = 12000,
)
```

An agent with an output schema must return exactly one JSON value as its final
answer. Rho parses the complete answer and validates it. It does not search for
JSON in prose or code fences. An agent with no schema may return prose, but a
condition or template cannot inspect that prose.

### Direct argv command

```starlark
command(
    name = "check",
    argv = ["cargo", "check", "-p", "rho-coding-agent"],
    cwd = ".",
    output = stdout_json(record({"passed": bool()})),
    timeout_seconds = 1800,
    max_output_bytes = 12000,
)
```

The first argv item is the executable. Later items may include typed output
references. Rho resolves and freezes the executable during planning. Command
nodes are always mutating because Rho has no OS sandbox that can enforce a
read-only claim for an arbitrary command.

### Explicit shell command

```starlark
shell(
    name = "check",
    executable = "bash",
    arguments = ["-lc"],
    command = "set -euo pipefail; cargo check",
    cwd = ".",
    timeout_seconds = 1800,
    max_output_bytes = 12000,
)
```

Shell execution is always explicit. Rho does not infer a shell from one string.
The shell executable, arguments, and command text are static. Do not place an
output reference in shell source.

## Schemas and output

Use these output schema constructors:

```starlark
null()
bool()
integer()
string()
enum(["approve", "revise"])
list(string())
optional(string())
record({
    "required": string(),
    "optional": optional(integer()),
})
stdout_json(record({"passed": bool()}))
```

Enum members must be scalar values. `stdout_json` parses the complete bounded
stdout after a direct or shell command ends. Invalid JSON or a schema mismatch
is a typed node failure.

## References and conditions

References must point to an ancestor node and a path allowed by its frozen
schema:

```starlark
output("inspect", ["decision"])
status("test")
exit_code("test")
```

Build conditions with:

```starlark
equals(output("inspect", ["decision"]), "approve")
is_one_of(status("test"), ["success", "failure"])
all([condition_a, condition_b])
any([condition_a, condition_b])
not(condition_a)
```

Node status values are `success`, `failure`, `denial`, `cancellation`,
`skipped`, and `blocked`. Exit conditions use an integer code or a supported
exit predicate. Do not branch on assistant prose, stdout text, substrings, or
regular expressions.

## Run and inspect

```bash
# Freeze a new immutable plan and record its ID and digest.
rho workflow plan .rho/workflows/review.star \
  --input 'package="rho-coding-agent"' --output text

# Run only by plan ID. In non-interactive use, consent must be explicit.
rho workflow run <PLAN_ID> --yes --output jsonl

# Read a durable snapshot without taking run ownership.
rho workflow status <RUN_ID> --output json

# Request cancellation. The owner stops active work before it marks it stopped.
rho workflow cancel <RUN_ID>

# Resume the frozen run. Successful nodes do not run again.
rho workflow resume <RUN_ID> --yes --output jsonl
```

The tool forms are:

```json
{"action":"plan","file":".rho/workflows/review.star","inputs":{"package":"rho-coding-agent"}}
{"action":"run","plan_id":"..."}
{"action":"status","run_id":"..."}
{"action":"cancel","run_id":"..."}
{"action":"resume","run_id":"..."}
```

Tool results contain bounded summaries, IDs, typed states, and artifact
references. They do not contain full source or logs. Read an artifact through
the normal file tools only when needed.

If a process ended without a clean owner handoff, the run enters
`needs_recovery`. Do not guess whether the process completed. Inspect the
attempt and use the recovery action shown by CLI help before resume.
