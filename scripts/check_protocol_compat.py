#!/usr/bin/env python3
"""Check rho-protocol's schema against the last release's (#1657).

A protocol minor may add methods, notifications, error codes, optional
fields, and values of open enums: those with a variant marked `x-rho-other`,
which matches every value the enum does not list. Anything else needs a new protocol major:
removing, retyping, or constraining a field, making a field required,
optional, or nullable, adding a required field to an existing object,
removing a value, or adding one to a closed enum. Any change at all needs a
new minor, so a client can tell from `rho/hello` what it may receive.

Major 0 is the unstable protocol (#1776): it may change anything, and a
change needs no new minor. Nothing of it is released while 3.0 is built, so
its minor may not rise: it stays put and is set once, at the beta. It may
fall, to reset the version earlier changes raised. The exception is the
handshake a client and daemon of different versions still exchange, the
version `rho/hello` names and the data of the error that refuses it, which
may only be added to at any major.

The baseline is the schema the last `rho-coding-agent` release shipped:
compatibility matters between released daemons and clients, not between
commits. Before the first release with a schema there is nothing to compare.
An unstable protocol has no release to hold it yet, and its users run builds
of any commit, so CI also compares each change with the commit before it
(`--baseline-ref HEAD^1 --unstable-only`).
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
# What a client and a daemon of different versions still read of each other:
# the version a hello names, and the refusal's data.
HANDSHAKE = (("method rho/hello params", ".protocol"), ("error -32001",))
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
    "prefixItems",
    "additionalProperties",
    OTHER,
}

# Where a fact applies: a root such as `method rho/hello params`, then
# segments: `.field`, `[]` (items), `[0]` (a tuple's first value), `{}` (map values), ` <value>` (the variant
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
    # The walk reads one kind of value per node: variants, a set of values,
    # or a type. Narrowing beside variants or values would go unread.
    kind = node.keys() & {"oneOf", "anyOf", "const", "enum"}
    beside = node.keys() & (CONSTRAINTS | {"items", "additionalProperties"})
    if kind & {"const", "enum"}:
        beside |= node.keys() & {"properties", "required"}
    if len(kind) > 1 or (kind and beside):
        together = ", ".join(sorted(kind | beside))
        raise ValueError(f"{render(path)}: the check does not model {together} together")
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
                check_other(node, variant, path)
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
    # A tuple: each position is its own value.
    for index, item in enumerate(node.get("prefixItems", [])):
        walk(defs, item, (*path, f"[{index}]"), out, refs)
    if isinstance(node.get("additionalProperties"), dict):
        walk(defs, node["additionalProperties"], (*path, "{}"), out, refs)


def check_other(node: dict, variant: dict, path: Where) -> None:
    """Fails unless `variant` matches every value its enum does not list:
    any string, or any object with a string tag, beside the other variants
    under `anyOf` (under `oneOf`, a listed value would match twice)."""
    shape = {key: value for key, value in variant.items() if key not in ANNOTATIONS | {OTHER}}
    properties = shape.get("properties", {})
    any_string = shape == {"type": "string"}
    any_tag = (
        shape.keys() == {"type", "properties", "required"}
        and shape["type"] == "object"
        and list(properties.values()) == [{"type": "string"}]
        and shape["required"] == list(properties)
    )
    if "anyOf" not in node or not (any_string or any_tag):
        raise ValueError(
            f"{render(path)}: its {OTHER} variant must be any string or any tagged object, "
            "under anyOf"
        )


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
            if key in ("properties", "required") or key in ANNOTATIONS:
                merged.setdefault(key, value)
            elif merged.setdefault(key, value) != value:
                raise ValueError(f"{render(path)}: the check does not model two values of {key}")
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
        if path in baseline or not isinstance(new, Required):
            continue
        # `path` is `(*parent, ".field", " required")`.
        parent = baseline.get(path[:-2])
        if parent is not None and new.required:
            problems.append(f"{render(path[:-1])}: added as required")
        elif isinstance(parent, Shape) and parent.closed:
            problems.append(f"{render(path[:-1])}: added to an object that refuses unknown fields")
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


def handshake(facts_: dict[Where, Fact]) -> dict[Where, Fact]:
    """The facts of `facts_` under [`HANDSHAKE`]."""
    return {
        path: fact
        for path, fact in facts_.items()
        if any(path[: len(prefix)] == prefix for prefix in HANDSHAKE)
    }


def check(baseline: dict, current: dict, baseline_name: str) -> list[str]:
    """Why `current` may not ship after `baseline`; empty when it may."""
    before, after = facts(baseline), facts(current)
    old, new = version(baseline), version(current)
    unstable = old[0] == new[0] == 0
    if new < old and not unstable:
        return [f"PROTOCOL_VERSION {new[0]}.{new[1]} is below {baseline_name}'s {old[0]}.{old[1]}"]
    if new[0] > old[0]:
        return []
    if unstable:
        # An unstable minor may change anything but the handshake.
        problems = [
            f"{problem} (clients and daemons of every version read the handshake, so it may "
            "only gain optional fields)"
            for problem in breaking_changes(handshake(before), handshake(after))
        ]
        if new[1] > old[1]:
            problems.append(
                f"PROTOCOL_VERSION rose from {old[0]}.{old[1]} to {new[0]}.{new[1]}; major 0 is "
                "unreleased, so its version stays put until the beta sets it once"
            )
    else:
        problems = [
            f"{problem} (needs a new protocol major)" for problem in breaking_changes(before, after)
        ]
    # An unstable protocol changes without a new minor until the beta.
    if before != after and new == old and new[0] > 0:
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
    Asks the remote because local tags may be stale. Fetches shallowly only
    into a clone that is already shallow, such as CI's: a shallow fetch
    makes a full clone, and every worktree sharing it, shallow. A tagged
    commit already in the clone, such as the release commit CI checks out,
    only gets its tag: fetching it shallowly would make it a shallow boundary
    and cut off the parent `--baseline-ref HEAD^1` compares against."""
    listing = git(
        "ls-remote",
        "--tags",
        REMOTE,
        "rho-coding-agent-v*",
        hint="offline, pass --baseline-ref with a local release tag",
    )
    # An annotated tag's peeled `<tag>^{}` line follows it and names its commit.
    commits = {}
    for line in listing.splitlines():
        commit, _, ref = line.partition("\t")
        commits[ref.removeprefix("refs/tags/").removesuffix("^{}")] = commit
    tag = latest_release(list(commits))
    if tag is None or exists(f"{tag}^{{commit}}"):
        return tag
    if exists(commits[tag]):
        git("tag", tag, commits[tag])
    else:
        shallow = git("rev-parse", "--is-shallow-repository").strip() == "true"
        depth = ["--depth=1"] if shallow else []
        git("fetch", "--no-tags", *depth, REMOTE, f"refs/tags/{tag}:refs/tags/{tag}")
    return tag


def exists(object_name: str) -> bool:
    return (
        subprocess.run(
            ["git", "cat-file", "-e", object_name], cwd=ROOT, capture_output=True
        ).returncode
        == 0
    )


def git(*arguments: str, hint: str | None = None) -> str:
    result = subprocess.run(["git", *arguments], cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        message = f"error: git {' '.join(arguments)} failed: {result.stderr.strip()}"
        raise SystemExit(f"{message}\n{hint}" if hint else message)
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
    parser.add_argument(
        "--unstable-only",
        action="store_true",
        help="compare only when both schemas are protocol major 0, as when holding each change "
        "to the commit before it",
    )
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
    if arguments.unstable_only and (version(baseline)[0], version(current)[0]) != (0, 0):
        # A stable protocol's minors count releases, not changes, and the
        # change that enters or leaves major 0 has no rule to break.
        print(f"skipped: {baseline_name} and the current schema are not both protocol major 0")
        return 0
    problems = check(baseline, current, baseline_name)
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    if problems:
        return 1
    new = version(current)
    print(f"protocol {new[0]}.{new[1]} may follow {baseline_name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
