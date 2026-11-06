#!/usr/bin/env python3
"""Check rho-protocol's schema against the last release's (#1657).

A protocol minor may add methods, notifications, error codes, optional
fields, and values of open enums: those with a variant marked `x-rho-other`,
which matches every value the enum does not list. Anything else needs a new protocol major:
removing, retyping, or constraining a field, making a field required,
optional, or nullable, adding a required field to an existing object,
removing a value, or adding one to a closed enum. Any change at all needs a
new minor, so a client can tell from `rho/hello` what it may receive.

The baseline is the schema the last `rho-coding-agent` release shipped:
compatibility matters between released daemons and clients, not between
commits. Before the first release with a schema there is nothing to compare.
Descriptions and Rust names are not part of the contract, so changing them
needs nothing.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = Path("crates/rho-protocol/schema/protocol.json")
RELEASE_TAG = re.compile(r"rho-coding-agent-v(\d+)\.(\d+)\.(\d+)")
REMOTE = "origin"
OTHER = "x-rho-other"
# Keys that describe a schema without constraining the wire.
ANNOTATIONS = {"description", "title", "default", "examples", "$comment", "deprecated"}
# Keys that narrow a type; any change to them is a change of the type.
CONSTRAINTS = {
    "format",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
}
# Every other key the walk understands. Any key outside these three sets
# fails the check, so a constraint the walk does not model is never ignored.
HANDLED = {
    "$ref",
    "allOf",
    "oneOf",
    "anyOf",
    "const",
    "enum",
    "type",
    "properties",
    "required",
    "items",
    "additionalProperties",
    OTHER,
}

# Where a fact applies: a root such as `method rho/hello params`, then
# segments: `.field`, `[]` (items), `{}` (map values), ` <value>` (the variant
# with that tag), and ` required`.
Where = tuple[str, ...]


class Shape(NamedTuple):
    """A value of one or more JSON types."""

    kinds: frozenset[str]
    constraints: tuple[tuple[str, str], ...]
    # `additionalProperties: false`: unknown fields are refused.
    closed: bool
    nullable: bool


class Values(NamedTuple):
    """One of a set of values or tagged variants."""

    labels: frozenset[str]
    open: bool
    nullable: bool


class AnyValue(NamedTuple):
    """Any JSON value."""


class Required(NamedTuple):
    required: bool


class Exists(NamedTuple):
    """An error code that exists."""


Fact = Shape | Values | AnyValue | Required | Exists


def facts(document: dict) -> dict[Where, Fact]:
    """Every wire constraint in `document`, keyed by where it applies, as
    clients see it: by method, notification, and field name, and errors by
    code, never by the Rust names in `$defs`."""
    defs = document.get("$defs", {})
    out: dict[Where, Fact] = {}
    for name, method in document.get("methods", {}).items():
        walk(defs, method["params"], (f"method {name} params",), out)
        walk(defs, method["result"], (f"method {name} result",), out)
    for name, notification in document.get("notifications", {}).items():
        walk(defs, notification["params"], (f"notification {name} params",), out)
    for code, error in document.get("errors", {}).items():
        out[(f"error {code}",)] = Exists()
        if "data" in error:
            walk(defs, error["data"], (f"error {code}", " data"), out)
    return out


def walk(
    defs: dict,
    node: object,
    path: Where,
    out: dict[Where, Fact],
    refs: frozenset[str] = frozenset(),
    nullable: bool = False,
) -> None:
    node, refs = resolve(defs, node, path, refs)
    if node is True or not node.keys() - ANNOTATIONS:
        out[path] = AnyValue()
        return
    unhandled = node.keys() - HANDLED - CONSTRAINTS - ANNOTATIONS
    if unhandled:
        raise ValueError(f"{render(path)}: the check does not model {', '.join(sorted(unhandled))}")
    if "oneOf" in node and "anyOf" in node:
        raise ValueError(f"{render(path)}: the check does not model oneOf with anyOf")
    variants = node.get("oneOf", node.get("anyOf"))
    if variants is not None:
        resolved = [resolve(defs, variant, path, refs) for variant in variants]
        others = [(variant, inner) for variant, inner in resolved if variant != {"type": "null"}]
        nullable = nullable or len(others) < len(resolved)
        # `Option<T>` is `anyOf [T, null]`: the same wire as a nullable T.
        if len(others) == 1 and node.keys() <= {"oneOf", "anyOf"} | ANNOTATIONS:
            variant, inner = others[0]
            walk(defs, variant, path, out, inner, nullable)
            return
        labels: set[str] = set()
        open_ = False
        for index, (variant, inner) in enumerate(others):
            # Matches every value the others do not; what it matches grows
            # with them, so it has no facts of its own.
            if isinstance(variant, dict) and variant.get(OTHER):
                open_ = True
                continue
            variant_values = variant_labels(variant, index)
            labels.update(variant_values)
            # A value is the whole of its variant; schemars groups values
            # without docs into one `enum`, so walking them would tie the
            # facts to how they are grouped.
            if isinstance(variant, dict) and ("const" in variant or "enum" in variant):
                continue
            for label in variant_values:
                walk(defs, variant, (*path, f" <{label}>"), out, inner)
        out[path] = Values(frozenset(labels), open_, nullable)
        # Fields beside the variants, as `#[serde(flatten)]` of an enum makes.
        walk_children(defs, node, path, out, refs)
        return
    if "const" in node:
        out[path] = Values(frozenset({json.dumps(node["const"])}), False, nullable)
        return
    if "enum" in node:
        labels = frozenset(json.dumps(value) for value in node["enum"] if value is not None)
        out[path] = Values(labels, False, nullable or None in node["enum"])
        return
    kinds = node.get("type", [])
    kinds = {kinds} if isinstance(kinds, str) else set(kinds)
    out[path] = Shape(
        kinds=frozenset(kinds - {"null"}),
        constraints=tuple(sorted((key, json.dumps(node[key])) for key in CONSTRAINTS & node.keys())),
        closed=node.get("additionalProperties") is False,
        nullable=nullable or "null" in kinds,
    )
    walk_children(defs, node, path, out, refs)


def walk_children(
    defs: dict, node: dict, path: Where, out: dict[Where, Fact], refs: frozenset[str]
) -> None:
    required = set(node.get("required", []))
    for name, field in node.get("properties", {}).items():
        out[(*path, f".{name}", " required")] = Required(name in required)
        walk(defs, field, (*path, f".{name}"), out, refs)
    if "items" in node:
        walk(defs, node["items"], (*path, "[]"), out, refs)
    if isinstance(node.get("additionalProperties"), dict):
        walk(defs, node["additionalProperties"], (*path, "{}"), out, refs)


def resolve(
    defs: dict, node: object, path: Where, refs: frozenset[str]
) -> tuple[object, frozenset[str]]:
    """`node` with its `$ref` and `allOf` parts merged in, and the `$defs`
    names followed on the way to it, so a recursive type fails loudly."""
    if not isinstance(node, dict):
        return node, refs
    merged = {key: value for key, value in node.items() if key not in ("$ref", "allOf")}
    parts = list(node.get("allOf", []))
    if "$ref" in node:
        name = node["$ref"].removeprefix("#/$defs/")
        if name in refs:
            raise ValueError(f"{render(path)}: {name} contains itself; the check does not model recursion")
        refs = refs | {name}
        parts.insert(0, defs[name])
    for part in parts:
        part, refs = resolve(defs, part, path, refs)
        if part is True:
            continue
        properties = {**part.get("properties", {}), **merged.get("properties", {})}
        required = sorted({*part.get("required", []), *merged.get("required", [])})
        for key, value in part.items():
            merged.setdefault(key, value)
        merged.pop("properties", None)
        merged.pop("required", None)
        if properties:
            merged["properties"] = properties
        if required:
            merged["required"] = required
    return merged, refs


def variant_labels(variant: object, index: int) -> list[str]:
    """How clients tell `variant` apart: its value, its tag, or, for an
    untagged union, its position."""
    if not isinstance(variant, dict):
        return [f"#{index}"]
    if "const" in variant:
        return [json.dumps(variant["const"])]
    if "enum" in variant:
        return [json.dumps(value) for value in variant["enum"]]
    required = set(variant.get("required", []))
    for name, field in variant.get("properties", {}).items():
        if name in required and isinstance(field, dict) and "const" in field:
            return [f"{name}={json.dumps(field['const'])}"]
    return [f"#{index}"]


def breaking_changes(baseline: dict[Where, Fact], current: dict[Where, Fact]) -> list[str]:
    """What `current` changed that a protocol minor may not."""
    problems = []
    for path, old in baseline.items():
        new = current.get(path)
        if new is None:
            problems.append(f"{render(path)}: removed")
        elif old == new:
            continue
        elif isinstance(old, Values) and isinstance(new, Values) and old._replace(labels=new.labels) == new:
            removed, added = old.labels - new.labels, new.labels - old.labels
            if removed:
                problems.append(f"{render(path)}: removed {', '.join(sorted(removed))}")
            if added and not old.open:
                problems.append(
                    f"{render(path)}: added {', '.join(sorted(added))} to a closed enum"
                )
        else:
            problems.append(f"{render(path)}: {describe(old)} became {describe(new)}")
    for path, new in current.items():
        # `path` is `(*parent, ".field", " required")`.
        if path not in baseline and new == Required(True) and path[:-2] in baseline:
            problems.append(f"{render(path[:-1])}: added as required")
    return problems


def render(path: Where) -> str:
    return "".join(path)


def describe(fact: Fact) -> str:
    match fact:
        case Required(required):
            return "required" if required else "optional"
        case Shape(kinds, constraints, closed, nullable):
            details = [f"{key} {value}" for key, value in constraints]
            details += ["no unknown fields"] * closed + ["nullable"] * nullable
            text = "/".join(sorted(kinds)) or "untyped"
            return f"{text} ({', '.join(details)})" if details else text
        case Values(labels, open_, nullable):
            details = ["open"] * open_ + ["nullable"] * nullable
            text = "one of " + ", ".join(sorted(labels))
            return f"{text} ({', '.join(details)})" if details else text
        case AnyValue():
            return "any value"
        case Exists():
            return "present"


def version(document: dict) -> tuple[int, int]:
    protocol = document["protocol"]
    return protocol["major"], protocol["minor"]


def check(baseline: dict, current: dict, baseline_name: str) -> list[str]:
    """Why `current` may not ship after `baseline`; empty when it may."""
    before, after = facts(baseline), facts(current)
    old, new = version(baseline), version(current)
    if new < old:
        return [f"PROTOCOL_VERSION {new[0]}.{new[1]} is below {baseline_name}'s {old[0]}.{old[1]}"]
    if new[0] > old[0]:
        return []
    problems = [
        f"{problem} (needs a new protocol major)" for problem in breaking_changes(before, after)
    ]
    if before != after and new == old:
        problems.append(
            f"the protocol changed since {baseline_name} but PROTOCOL_VERSION is still "
            f"{new[0]}.{new[1]}; raise the minor"
        )
    return problems


def latest_release(tags: list[str]) -> str | None:
    """The newest release among `tags`, by version rather than by text.
    Tags that are not `rho-coding-agent-vX.Y.Z` are not releases."""
    releases = {}
    for tag in tags:
        if match := RELEASE_TAG.fullmatch(tag):
            releases[tuple(map(int, match.groups()))] = tag
    return releases[max(releases)] if releases else None


def fetch_latest_release() -> str | None:
    """The newest release tag on the remote, fetched if it is not local.
    Asks the remote because local tags may be stale."""
    listing = git("ls-remote", "--tags", "--refs", REMOTE, "rho-coding-agent-v*")
    tag = latest_release([line.split("refs/tags/", 1)[1] for line in listing.splitlines()])
    if tag is not None and not exists(f"{tag}^{{commit}}"):
        git("fetch", "--no-tags", "--depth=1", REMOTE, f"refs/tags/{tag}:refs/tags/{tag}")
    return tag


def exists(object_name: str) -> bool:
    return (
        subprocess.run(
            ["git", "cat-file", "-e", object_name], cwd=ROOT, capture_output=True
        ).returncode
        == 0
    )


def git(*arguments: str) -> str:
    result = subprocess.run(["git", *arguments], cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"error: git {' '.join(arguments)} failed: {result.stderr.strip()}")
    return result.stdout


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    baseline_source = parser.add_mutually_exclusive_group()
    baseline_source.add_argument("--baseline", type=Path, help="baseline schema file")
    baseline_source.add_argument(
        "--baseline-ref",
        help=f"git ref to read the baseline from (default: the newest release tag on {REMOTE})",
    )
    parser.add_argument("--current", type=Path, default=ROOT / SCHEMA, help="schema to check")
    arguments = parser.parse_args(argv)

    if arguments.baseline is not None:
        baseline_name = str(arguments.baseline)
        baseline_text = arguments.baseline.read_text()
    else:
        baseline_name = arguments.baseline_ref or fetch_latest_release()
        if baseline_name is None:
            print(f"{REMOTE} has no rho-coding-agent release tag; nothing to compare")
            return 0
        if not exists(f"{baseline_name}^{{commit}}"):
            print(f"error: {baseline_name} is not a commit", file=sys.stderr)
            return 1
        if not exists(f"{baseline_name}:{SCHEMA.as_posix()}"):
            print(f"{baseline_name} has no {SCHEMA}; nothing to compare")
            return 0
        baseline_text = git("show", f"{baseline_name}:{SCHEMA.as_posix()}")

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
