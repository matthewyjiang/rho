#!/usr/bin/env python3
"""Generate the deterministic workflow limit acceptance corpus."""

from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class CorpusCase:
    name: str
    entry: Path
    call_stack_depth: int = 0
    parser_list_items: int = 0


def _node(
    name: str,
    *,
    argv: str = '["./wait.sh", "-c", "true"]',
    needs: str = "[]",
    when: str = "None",
    output: str = "None",
    timeout: int = 60,
    max_output: int = 1,
) -> str:
    return (
        f'command(name = "{name}", argv = {argv}, cwd = ".", '
        f"needs = {needs}, when = {when}, output = {output}, "
        f"timeout_seconds = {timeout}, max_output_bytes = {max_output})"
    )


def _workflow(body: str, *, inputs: str = "{}") -> str:
    return (
        f"{body}\n\n"
        "def build(inputs):\n"
        "    return workflow(name = \"limit_acceptance\", nodes = NODES)\n\n"
        f"WORKFLOW = define(inputs = {inputs}, build = build)\n"
    )


def _write(path: Path, source: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source)


def _source_module_case(root: Path) -> CorpusCase:
    case = root / "source_modules"
    module_count = 75
    chain_modules = 14
    for index in range(module_count - 1):
        load = ""
        if index + 1 < chain_modules:
            load = f'load("//source_modules/module_{index + 1:02}.star", "value_{index + 1:02}")\n'
        _write(case / f"module_{index:02}.star", f"{load}value_{index:02} = {index}\n")
    loads = "".join(
        f'load("//source_modules/module_{index:02}.star", "value_{index:02}")\n'
        for index in range(module_count - 1)
    )
    entry = case / "entry.star"
    _write(entry, loads + _workflow("NODES = []"))

    target_bytes = 750_000
    paths = sorted(case.glob("*.star"))
    current = sum(path.stat().st_size for path in paths)
    padding = target_bytes - current
    if padding < 2:
        raise RuntimeError("source module fixture exceeds its byte target")
    with (case / "module_73.star").open("a") as output:
        output.write("#" + ("p" * (padding - 2)) + "\n")
    actual = sum(path.stat().st_size for path in paths)
    if actual != target_bytes:
        raise RuntimeError(f"source fixture is {actual} bytes, expected {target_bytes}")
    return CorpusCase("source_modules", entry)


def _evaluator_case(root: Path) -> CorpusCase:
    case = root / "evaluator_values"
    call_depth = 15
    functions = []
    for index in reversed(range(call_depth)):
        result = "0" if index == call_depth - 1 else f"call_{index + 1}()"
        functions.append(f"def call_{index}():\n    return {result}\n")
    source = "\n".join(reversed(functions)) + "\n"
    source += (
        "def make_nodes():\n"
        "    call_result = call_0()\n"
        "    tick_result = 0\n"
        "    for item in range(735000):\n"
        "        tick_result = item\n"
        "    heap_result = [(\"h\" * 3000) + str(item) for item in range(7500)]\n"
        "    large_string = \"s\" * 750000\n"
        "    return [" + _node(
            "values",
            argv='["./wait.sh"] + [large_string] + ([""] * 7498)',
        ) + "]\n"
        "NODES = make_nodes()\n"
    )
    entry = case / "entry.star"
    _write(entry, _workflow(source))
    return CorpusCase(
        "evaluator_values",
        entry,
        call_stack_depth=call_depth,
        parser_list_items=7500,
    )


def _graph_edges_case(root: Path) -> CorpusCase:
    case = root / "graph_edges"
    source = (
        "def make_node(index):\n"
        "    first = max(0, index - 10)\n"
        "    dependencies = [\"node_\" + str(item) for item in range(first, index)]\n"
        "    dependencies += [\"node_0\"] if index >= 100 and index < 155 else []\n"
        "    return command(\n"
        "        name = \"node_\" + str(index),\n"
        "        argv = [\"./wait.sh\", \"-c\", \"true\"],\n"
        "        cwd = \".\", needs = dependencies,\n"
        "        timeout_seconds = 60, max_output_bytes = 1,\n"
        "    )\n"
        "NODES = [make_node(index) for index in range(750)]\n"
    )
    entry = case / "entry.star"
    _write(entry, _workflow(source))
    return CorpusCase("graph_edges", entry)


def _schema_condition_case(root: Path) -> CorpusCase:
    case = root / "schema_condition"
    deep_schema = "string()"
    condition = 'equals(status("root"), "success")'
    for _ in range(3):
        deep_schema = f"list({deep_schema})"
    source = (
        "FIELDS = {\"field_\" + str(index) + \"_\" + (\"k\" * 41): string() "
        "for index in range(7500)}\n"
        f'FIELDS["deep"] = {deep_schema}\n'
        "NODES = [\n"
        "    " + _node("root", output="stdout_json(record(FIELDS))") + ",\n"
        "    " + _node("conditional", needs='["root"]', when=condition) + ",\n"
        "]\n"
    )
    entry = case / "entry.star"
    _write(entry, _workflow(source))
    return CorpusCase("schema_condition", entry)


def _graph_bytes_case(root: Path) -> CorpusCase:
    case = root / "graph_bytes"
    source = (
        "NODES = [command(\n"
        "    name = \"graph_\" + str(index),\n"
        "    argv = [\"./wait.sh\", (\"g\" * 9700) + str(index)],\n"
        "    cwd = \".\", timeout_seconds = 60, max_output_bytes = 1,\n"
        ") for index in range(750)]\n"
    )
    entry = case / "entry.star"
    _write(entry, _workflow(source))
    return CorpusCase("graph_bytes", entry)


def _runtime_output_case(root: Path) -> CorpusCase:
    case = root / "runtime_output"
    nodes = ",\n    ".join(
        _node(
            f"output_{index}",
            timeout=64_800 if index == 0 else 60,
            max_output=6_291_456,
        )
        for index in range(4)
    )
    entry = case / "entry.star"
    _write(entry, _workflow(f"NODES = [\n    {nodes},\n]"))
    return CorpusCase("runtime_output", entry)


def _prompt_schema_case(root: Path) -> CorpusCase:
    case = root / "prompt_schema"
    source = (
        "FIELDS = {\"prompt_\" + str(index) + \"_\" + (\"p\" * 41): string() "
        "for index in range(7500)}\n"
        "NODES = [\n"
        "command(name = \"prompt_source\", argv = [\"./wait.sh\"], cwd = \".\", "
        "output = stdout_json(string()), timeout_seconds = 60, max_output_bytes = 3145728),\n"
        "agent(\n"
        "    name = \"prompt\", agent = \"reviewer\", needs = [\"prompt_source\"],\n"
        "    prompt = template([output(\"prompt_source\", [])]), access = \"mutating\",\n"
        "    output = record(FIELDS), timeout_seconds = 60, max_output_bytes = 1,\n"
        ")]\n"
    )
    entry = case / "entry.star"
    _write(entry, _workflow(source))
    return CorpusCase("prompt_schema", entry)


def _argv_case(root: Path) -> CorpusCase:
    case = root / "argv_expansion"
    source = (
        "NODES = [\n"
        "command(name = \"argv_source\", argv = [\"./wait.sh\"], cwd = \".\", "
        "output = stdout_json(string()), timeout_seconds = 60, max_output_bytes = 3145700),\n"
        + _node(
            "argv",
            argv='["./wait.sh", template([output("argv_source", [])]), template([output("argv_source", [])])]',
            needs='["argv_source"]',
        )
        + ",\n]\n"
    )
    entry = case / "entry.star"
    _write(entry, _workflow(source))
    return CorpusCase("argv_expansion", entry)


def _input_case(root: Path) -> CorpusCase:
    case = root / "input_bytes"
    source = (
        "INPUT_DEFAULT = \"i\" * 749900\n"
        "NODES = [" + _node("input", argv='["./wait.sh", INPUT_DEFAULT]') + "]\n"
    )
    # INPUT_DEFAULT must exist before WORKFLOW evaluates its input declaration.
    entry = case / "entry.star"
    _write(entry, _workflow(source, inputs='{"payload": input.string(default = INPUT_DEFAULT)}'))
    return CorpusCase("input_bytes", entry)


def generate_corpus(root: Path) -> list[CorpusCase]:
    root.mkdir(parents=True, exist_ok=True)
    executable = root / "wait.sh"
    executable.write_text("#!/bin/sh\nexit 0\n")
    os.chmod(executable, 0o700)
    return [
        _source_module_case(root),
        _evaluator_case(root),
        _graph_edges_case(root),
        _schema_condition_case(root),
        _graph_bytes_case(root),
        _runtime_output_case(root),
        _prompt_schema_case(root),
        _argv_case(root),
        _input_case(root),
    ]


def source_request(case: CorpusCase, corpus_root: Path) -> tuple[str, dict[str, str], dict]:
    sources: dict[str, str] = {}
    modules: dict[str, dict[str, object]] = {}
    for path in sorted(case.entry.parent.glob("*.star")):
        relative = path.relative_to(corpus_root).as_posix()
        label = f"//{relative}"
        source = path.read_text()
        sources[label] = source
        modules[label] = {
            "digest": f"sha256:{hashlib.sha256(source.encode()).hexdigest()}",
            "bytes": len(source.encode()),
        }
    entry_label = f"//{case.entry.relative_to(corpus_root).as_posix()}"
    manifest = {"entry_label": entry_label, "modules": modules}
    return entry_label, sources, manifest
