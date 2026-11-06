#!/usr/bin/env python3
"""Check rho-protocol's schema against the last release's (#1657).

A protocol minor may add methods, notifications, error codes, optional
fields, and values of enums that read unknown values as `unknown`. Anything
else needs a new protocol major: removing or retyping a field, making a
field required or optional, adding a required field to an existing object,
removing a value, or adding one to a closed enum. Any change at all needs a
new minor, so a client can tell from `rho/hello` what it may receive.

The baseline is the schema the last `rho-coding-agent` release shipped:
compatibility matters between released daemons and clients, not between
commits. Before the first release with a schema there is nothing to compare.
Descriptions are not part of the contract, so changing them needs nothing.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = Path("crates/rho-protocol/schema/protocol.json")
RELEASE_TAGS = "rho-coding-agent-v*"
# Keys that describe a schema without constraining the wire.
ANNOTATIONS = {"description", "title", "default", "examples", "$comment", "deprecated"}
# Deep enough for every protocol type; deeper means a recursive type.
MAX_DEPTH = 64

Fact = tuple


def facts(document: dict) -> dict[str, Fact]:
    """Every wire constraint in `document`, keyed by where it applies, as
    clients see it: by method, notification, and field name, never by the
    Rust type names in `$defs`."""
    defs = document.get("$defs", {})
    out: dict[str, Fact] = {}
    for name, method in document.get("methods", {}).items():
        walk(defs, method["params"], f"method {name} params", out, 0)
        walk(defs, method["result"], f"method {name} result", out, 0)
    for name, notification in document.get("notifications", {}).items():
        walk(defs, notification["params"], f"notification {name} params", out, 0)
    for name, error in document.get("errors", {}).items():
        out[f"error {name}"] = ("code", error["code"])
        if "data" in error:
            walk(defs, error["data"], f"error {name} data", out, 0)
    return out


def walk(defs: dict, node: object, path: str, out: dict[str, Fact], depth: int) -> None:
    if depth > MAX_DEPTH:
        raise ValueError(f"{path}: nested over {MAX_DEPTH} levels; is a type recursive?")
    node = resolve(defs, node)
    if node is True or (isinstance(node, dict) and not node.keys() - ANNOTATIONS):
        out[path] = ("any",)
        return
    variants = node.get("oneOf") or node.get("anyOf")
    if variants is not None:
        labels: set[str] = set()
        for index, variant in enumerate(variants):
            variant = resolve(defs, variant)
            variant_values = variant_labels(variant, index)
            labels.update(variant_values)
            # A value is the whole of its variant; schemars groups values
            # without docs into one `enum`, so walking them would tie the
            # facts to how they are grouped.
            if isinstance(variant, dict) and ("const" in variant or "enum" in variant):
                continue
            for label in variant_values:
                walk(defs, variant, f"{path} <{label}>", out, depth + 1)
        out[path] = ("values", frozenset(labels))
        return
    if "const" in node:
        out[path] = ("const", json.dumps(node["const"]))
        return
    if "enum" in node:
        out[path] = ("values", frozenset(json.dumps(value) for value in node["enum"]))
        return
    kinds = node.get("type", [])
    kinds = frozenset([kinds] if isinstance(kinds, str) else kinds)
    out[path] = ("type", kinds, node.get("format"))
    required = set(node.get("required", []))
    for name, field in node.get("properties", {}).items():
        out[f"{path}.{name} required"] = ("required", name in required)
        walk(defs, field, f"{path}.{name}", out, depth + 1)
    if "items" in node:
        walk(defs, node["items"], f"{path}[]", out, depth + 1)
    if isinstance(node.get("additionalProperties"), dict):
        walk(defs, node["additionalProperties"], f"{path}{{}}", out, depth + 1)


def resolve(defs: dict, node: object) -> object:
    """`node` with its `$ref` and `allOf` parts merged in."""
    if not isinstance(node, dict):
        return node
    merged = {key: value for key, value in node.items() if key not in ("$ref", "allOf")}
    parts = list(node.get("allOf", []))
    if "$ref" in node:
        parts.insert(0, defs[node["$ref"].removeprefix("#/$defs/")])
    for part in parts:
        part = resolve(defs, part)
        if part is True:
            continue
        merged["properties"] = {**part.get("properties", {}), **merged.get("properties", {})}
        merged["required"] = sorted({*part.get("required", []), *merged.get("required", [])})
        for key, value in part.items():
            merged.setdefault(key, value)
        if not merged["properties"]:
            del merged["properties"]
        if not merged["required"]:
            del merged["required"]
    return merged


def variant_labels(variant: object, index: int) -> list[str]:
    """How clients tell `variant` apart: its value, its tag, or `null`."""
    if not isinstance(variant, dict):
        return [f"#{index}"]
    if "const" in variant:
        return [json.dumps(variant["const"])]
    if "enum" in variant:
        return [json.dumps(value) for value in variant["enum"]]
    if variant.get("type") == "null":
        return ["null"]
    required = set(variant.get("required", []))
    for name, field in variant.get("properties", {}).items():
        if name in required and isinstance(field, dict) and "const" in field:
            return [f"{name}={json.dumps(field['const'])}"]
    return [f"#{index}"]


def is_open(labels: frozenset[str]) -> bool:
    """Whether an enum reads values it does not know as `unknown`."""
    return any(label == '"unknown"' or label.endswith('="unknown"') for label in labels)


def breaking_changes(baseline: dict[str, Fact], current: dict[str, Fact]) -> list[str]:
    """What `current` changed that a protocol minor may not."""
    problems = []
    for path, old in baseline.items():
        new = current.get(path)
        if new is None:
            problems.append(f"{path}: removed")
        elif old != new:
            if old[0] == new[0] == "values":
                removed, added = old[1] - new[1], new[1] - old[1]
                if removed:
                    problems.append(f"{path}: removed {', '.join(sorted(removed))}")
                if added and not is_open(old[1]):
                    problems.append(
                        f"{path}: added {', '.join(sorted(added))} to an enum without `unknown`"
                    )
            else:
                problems.append(f"{path}: {describe(old)} became {describe(new)}")
    for path, new in current.items():
        if path in baseline or new != ("required", True):
            continue
        field = path.removesuffix(" required")
        if field.rsplit(".", 1)[0] in baseline:
            problems.append(f"{field}: added as required")
    return problems


def describe(fact: Fact) -> str:
    match fact:
        case ("required", required):
            return "required" if required else "optional"
        case ("type", kinds, format_):
            text = "/".join(sorted(kinds)) or "untyped"
            return f"{text} ({format_})" if format_ else text
        case ("values", labels):
            return "one of " + ", ".join(sorted(labels))
        case _:
            return " ".join(str(part) for part in fact)


def version(document: dict) -> tuple[int, int]:
    protocol = document["protocol"]
    return protocol["major"], protocol["minor"]


def check(baseline: dict, current: dict, baseline_name: str) -> list[str]:
    """Why `current` may not ship after `baseline`; empty when it may."""
    old, new = version(baseline), version(current)
    if new < old:
        return [f"PROTOCOL_VERSION {new[0]}.{new[1]} is below {baseline_name}'s {old[0]}.{old[1]}"]
    if new[0] > old[0]:
        return []
    before, after = facts(baseline), facts(current)
    problems = [
        f"{problem} (needs a new protocol major)" for problem in breaking_changes(before, after)
    ]
    if before != after and new == old:
        problems.append(
            f"the protocol changed since {baseline_name} but PROTOCOL_VERSION is still "
            f"{new[0]}.{new[1]}; raise the minor"
        )
    return problems


def latest_release() -> str | None:
    tags = git("tag", "--list", RELEASE_TAGS, "--sort=-v:refname").split()
    return tags[0] if tags else None


def git(*arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    baseline_source = parser.add_mutually_exclusive_group()
    baseline_source.add_argument("--baseline", type=Path, help="baseline schema file")
    baseline_source.add_argument(
        "--baseline-ref", help=f"git ref to read the baseline from (default: newest {RELEASE_TAGS} tag)"
    )
    parser.add_argument("--current", type=Path, default=ROOT / SCHEMA, help="schema to check")
    arguments = parser.parse_args(argv)

    if arguments.baseline is not None:
        baseline_name = str(arguments.baseline)
        baseline_text = arguments.baseline.read_text()
    else:
        baseline_name = arguments.baseline_ref or latest_release()
        if baseline_name is None:
            print(f"no {RELEASE_TAGS} tag; nothing to compare")
            return 0
        try:
            baseline_text = git("show", f"{baseline_name}:{SCHEMA.as_posix()}")
        except subprocess.CalledProcessError:
            print(f"{baseline_name} has no {SCHEMA}; nothing to compare")
            return 0

    baseline = json.loads(baseline_text)
    current = json.loads(arguments.current.read_text())
    problems = check(baseline, current, baseline_name)
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    if problems:
        return 1
    new = version(current)
    print(f"protocol {new[0]}.{new[1]} is compatible with {baseline_name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
