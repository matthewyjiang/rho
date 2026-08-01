#!/usr/bin/env python3
"""Measure and verify the workflow limit acceptance receipts.

The command generates each corpus case in a temporary workspace, sends it
through the product planner worker, and derives each named budget from the
returned plan. It also runs the public validate command for every case.
"""

from __future__ import annotations

import argparse
import ast
import json
import math
import os
import re
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

from workflow_limit_corpus import CorpusCase, generate_corpus, source_request

ROOT = Path(__file__).resolve().parents[1]
RECEIPT_PATH = ROOT / "crates/rho/src/workflow/fixtures/limit_receipt.json"
WORKFLOW_CLI_PATH = ROOT / "crates/rho/src/app/workflow_cli.rs"
WORKFLOW_MODEL_PATH = ROOT / "crates/rho/src/workflow/model.rs"
WORKFLOW_EXACT_PROCESS_PATH = ROOT / "crates/rho/src/tools/process/exact.rs"
WORKFLOW_CANCELLATION_PATH = (
    ROOT / "crates/rho/src/app/workflow_runtime/cancellation.rs"
)
TOKEN_BYTES = 32
DETERMINISTIC_FIELDS = {
    "total_source_bytes",
    "module_count",
    "module_depth",
    "evaluator_ticks",
    "evaluator_heap_bytes",
    "call_stack_depth",
    "string_bytes",
    "list_items",
    "dict_items",
    "input_depth",
    "input_bytes",
    "node_count",
    "edge_count",
    "condition_depth",
    "schema_depth",
    "schema_bytes",
    "graph_bytes",
    "retained_output_per_stream_bytes",
    "retained_output_total_bytes",
    "rendered_template_bytes",
    "node_timeout_seconds",
    "prompt_expansion_bytes",
    "argv_expansion_bytes",
}
# Workflow schema v1 forbids source-controlled environment entries. The
# receipt's zero baseline is a schema sentinel, not a corpus measurement.
UNMEASURED_FIELDS = {"environment_expansion_bytes"}


def compact_json(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def integer_expression_from(source: str, name: str, path: Path) -> int:
    match = re.search(rf"const {name}:[^=]+=(.*?);", source)
    if not match:
        raise SystemExit(f"Rust constant not found: {name}")
    tree = ast.parse(match.group(1).strip(), mode="eval")
    allowed = (
        ast.Expression,
        ast.BinOp,
        ast.Constant,
        ast.Mult,
        ast.Add,
        ast.Sub,
        ast.FloorDiv,
    )
    if any(not isinstance(node, allowed) for node in ast.walk(tree)):
        raise SystemExit(f"unsupported Rust constant expression for {name}")
    return int(eval(compile(tree, str(path), "eval"), {"__builtins__": {}}))


def integer_expression(source: str, name: str) -> int:
    return integer_expression_from(source, name, WORKFLOW_CLI_PATH)


def process_address_space_bytes(pid: int) -> int:
    status = Path(f"/proc/{pid}/status")
    try:
        text = status.read_text()
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        return 0
    match = re.search(r"^VmSize:\s+(\d+)\s+kB$", text, re.MULTILINE)
    return int(match.group(1)) * 1024 if match else 0


def child_pids(pid: int) -> list[int]:
    children = Path(f"/proc/{pid}/task/{pid}/children")
    try:
        return [int(child) for child in children.read_text().split()]
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        return []


def is_planner_worker(pid: int) -> bool:
    environment = Path(f"/proc/{pid}/environ")
    try:
        values = environment.read_bytes().split(b"\0")
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        return False
    return any(value.startswith(b"RHO_WORKFLOW_PLANNER_WORKER=") for value in values)


def run_worker(rho: Path, request: dict[str, Any], home: Path) -> tuple[dict, dict]:
    token = request["token"]
    payload = compact_json(request)
    frame = len(payload).to_bytes(8, "big") + payload
    env = os.environ.copy()
    env.update({"RHO_HOME": str(home), "RHO_WORKFLOW_PLANNER_WORKER": token})
    started = time.monotonic_ns()
    process = subprocess.Popen(
        [str(rho), "__workflow_planner_worker"],
        cwd=home,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout, stderr = process.communicate(frame)
    wall_millis = math.ceil((time.monotonic_ns() - started) / 1_000_000)
    if process.returncode != 0:
        raise SystemExit(
            f"planner worker failed ({process.returncode})\n"
            f"stdout:\n{stdout.decode(errors='replace')}\n"
            f"stderr:\n{stderr.decode(errors='replace')}"
        )
    if len(stdout) < 8:
        raise SystemExit("planner worker returned no framed response")
    size = int.from_bytes(stdout[:8], "big")
    if size != len(stdout) - 8:
        raise SystemExit(f"planner worker response frame says {size}, got {len(stdout) - 8}")
    response = json.loads(stdout[8:])
    if response.get("error") is not None or response.get("plan") is None:
        raise SystemExit(f"planner rejected acceptance corpus: {response.get('error')}")
    return response["plan"], {
        "request_bytes": len(payload),
        "response_bytes": size,
        "stderr_bytes": len(stderr),
        "worker_wall_millis": wall_millis,
    }


def run_public_validation(
    rho: Path, case: CorpusCase, corpus_root: Path, home: Path
) -> tuple[int, int]:
    started = time.monotonic_ns()
    process = subprocess.Popen(
        [str(rho), "workflow", "validate", str(case.entry.relative_to(corpus_root))],
        cwd=corpus_root,
        env={**os.environ, "RHO_HOME": str(home)},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    result: dict[str, bytes] = {}

    def communicate() -> None:
        stdout, stderr = process.communicate()
        result["stdout"] = stdout
        result["stderr"] = stderr

    thread = threading.Thread(target=communicate)
    thread.start()
    peak_address_space = 0
    while thread.is_alive():
        for child in child_pids(process.pid):
            if not is_planner_worker(child):
                continue
            peak_address_space = max(
                peak_address_space, process_address_space_bytes(child)
            )
        thread.join(0.001)
    thread.join()
    elapsed = math.ceil((time.monotonic_ns() - started) / 1_000_000)
    if process.returncode != 0:
        raise SystemExit(
            f"public validation failed for {case.name}\n"
            f"stdout:\n{result['stdout'].decode(errors='replace')}\n"
            f"stderr:\n{result['stderr'].decode(errors='replace')}"
        )
    return elapsed, peak_address_space


def value_depth(value: Any) -> int:
    if isinstance(value, list):
        return 1 + max((value_depth(item) for item in value), default=0)
    if isinstance(value, dict):
        return 1 + max((value_depth(item) for item in value.values()), default=0)
    return 1


def collection_maxima(value: Any) -> tuple[int, int, int]:
    strings = lists = dictionaries = 0
    pending = [value]
    while pending:
        item = pending.pop()
        if isinstance(item, str):
            strings = max(strings, len(item.encode()))
        elif isinstance(item, list):
            lists = max(lists, len(item))
            pending.extend(item)
        elif isinstance(item, dict):
            dictionaries = max(dictionaries, len(item))
            for key, child in item.items():
                strings = max(strings, len(key.encode()))
                pending.append(child)
    return strings, lists, dictionaries


def schema_depth(schema: dict[str, Any]) -> int:
    if schema["type"] == "list":
        return 1 + schema_depth(schema["item"])
    if schema["type"] == "object":
        return 1 + max(
            (schema_depth(field["schema"]) for field in schema["fields"].values()),
            default=0,
        )
    return 1


def condition_depth(condition: dict[str, Any]) -> int:
    kind = condition["type"]
    if kind == "not":
        return 1 + condition_depth(condition["condition"])
    if kind in ("all", "any"):
        return 1 + max(map(condition_depth, condition["conditions"]), default=0)
    return 1


def template_bytes(template: list[dict[str, Any]], nodes: dict[str, Any]) -> int:
    total = 0
    for part in template:
        if part["type"] == "literal":
            total += len(part["value"].encode())
        else:
            total += nodes[part["reference"]["node"]]["max_output_bytes"]
    return total


def plan_measurements(plan: dict[str, Any]) -> dict[str, int]:
    graph = plan["graph"]
    nodes = graph["nodes"]
    schemas = []
    conditions = []
    retained_total = 0
    rendered_max = prompt_max = argv_max = 0
    timeout_max = output_max = 0
    for node in nodes.values():
        execution = node["execution"]
        timeout_max = max(timeout_max, node["timeout_seconds"])
        output_max = max(output_max, node["max_output_bytes"])
        retained_total += node["max_output_bytes"] * (
            2 if execution["kind"] == "command" else 1
        )
        if node.get("condition") is not None:
            conditions.append(node["condition"])
        schema = execution.get("output")
        if schema is not None:
            schemas.append(schema)
        if execution["kind"] == "agent":
            rendered = template_bytes(execution["prompt"], nodes)
            rendered_max = max(rendered_max, rendered)
            prompt_max = max(
                prompt_max,
                rendered + (len(compact_json(schema)) if schema is not None else 0),
            )
        elif execution["invocation"] == "direct":
            rendered_arguments = [
                template_bytes(argument, nodes) for argument in execution["arguments"]
            ]
            rendered_max = max(rendered_max, *rendered_arguments, 0)
            argv_max = max(
                argv_max,
                len(execution["executable"].encode()) + sum(rendered_arguments),
            )
        else:
            argv_max = max(
                argv_max,
                len(execution["executable"].encode())
                + len(execution["command"].encode())
                + sum(len(argument.encode()) for argument in execution["arguments"]),
            )
    strings, lists, dictionaries = collection_maxima({"graph": graph, "inputs": plan["inputs"]})
    return {
        "evaluator_ticks": plan["evaluator_ticks"],
        "evaluator_heap_bytes": plan["evaluator_peak_heap_bytes"],
        "string_bytes": strings,
        "list_items": lists,
        "dict_items": dictionaries,
        "input_depth": max((value_depth(value) for value in plan["inputs"].values()), default=1),
        "input_bytes": len(compact_json(plan["inputs"])),
        "node_count": len(nodes),
        "edge_count": sum(len(node["needs"]) for node in nodes.values()),
        "condition_depth": max(map(condition_depth, conditions), default=0),
        "schema_depth": max(map(schema_depth, schemas), default=0),
        "schema_bytes": max((len(compact_json(schema)) for schema in schemas), default=0),
        "graph_bytes": len(compact_json(graph)),
        "retained_output_per_stream_bytes": output_max,
        "retained_output_total_bytes": retained_total,
        "rendered_template_bytes": rendered_max,
        "node_timeout_seconds": timeout_max,
        "prompt_expansion_bytes": prompt_max,
        "argv_expansion_bytes": argv_max,
    }


def module_depth(sources: dict[str, str], entry: str) -> int:
    loads = {
        label: re.findall(r'^load\("([^"]+)"', source, re.MULTILINE)
        for label, source in sources.items()
    }

    def depth(label: str, seen: frozenset[str]) -> int:
        if label in seen:
            raise SystemExit(f"cycle in generated corpus at {label}")
        return 1 + max(
            (depth(child, seen | {label}) for child in loads[label]), default=0
        )

    return depth(entry, frozenset())


def measure_corpus(rho: Path, *, public_validation: bool) -> tuple[dict, dict, dict]:
    maxima = {name: 0 for name in DETERMINISTIC_FIELDS | {"worker_wall_millis"}}
    process_maxima = {
        "request_frame_bytes": 0,
        "response_frame_bytes": 0,
        "stderr_bytes": 0,
        "address_space_bytes": 0,
    }
    cases_report: dict[str, dict[str, int]] = {}
    with tempfile.TemporaryDirectory(prefix="rho-workflow-limit-") as directory:
        workspace = Path(directory)
        corpus_root = workspace / "corpus"
        cases = generate_corpus(corpus_root)
        for index, case in enumerate(cases):
            entry, sources, manifest = source_request(case, corpus_root)
            request = {
                "token": f"{index + 1:064x}",
                "entry_label": entry,
                "sources": sources,
                "manifest": manifest,
                "inputs": {},
            }
            home = workspace / f"home-{case.name}"
            home.mkdir()
            plan, process = run_worker(rho, request, home)
            measured = plan_measurements(plan)
            measured.update(
                {
                    "total_source_bytes": sum(len(value.encode()) for value in sources.values()),
                    "module_count": len(sources),
                    "module_depth": module_depth(sources, entry),
                    "call_stack_depth": case.call_stack_depth,
                }
            )
            measured["list_items"] = max(
                measured["list_items"], case.parser_list_items
            )
            public_address_space = 0
            if public_validation:
                _, public_address_space = run_public_validation(
                    rho, case, corpus_root, home
                )
            measured["worker_wall_millis"] = process["worker_wall_millis"]
            for name, value in measured.items():
                maxima[name] = max(maxima[name], value)
            for receipt_name, process_name in [
                ("request_frame_bytes", "request_bytes"),
                ("response_frame_bytes", "response_bytes"),
                ("stderr_bytes", "stderr_bytes"),
            ]:
                process_maxima[receipt_name] = max(
                    process_maxima[receipt_name], process[process_name]
                )
            process_maxima["address_space_bytes"] = max(
                process_maxima["address_space_bytes"], public_address_space
            )
            cases_report[case.name] = measured
            print(
                f"case {case.name}: ticks={measured['evaluator_ticks']} "
                f"heap={measured['evaluator_heap_bytes']} graph={measured['graph_bytes']} "
                f"wall={measured['worker_wall_millis']}ms"
            )
    return maxima, process_maxima, cases_report


def load_receipt() -> dict:
    receipt = json.loads(RECEIPT_PATH.read_text())
    if receipt["schema_version"] != 2:
        raise SystemExit(f"unsupported receipt schema: {receipt['schema_version']}")
    return receipt


def verify_arithmetic(receipt: dict) -> None:
    for section_name in ("planning",):
        section = receipt[section_name]
        names = set(section["accepted"]) - {"receipt"}
        if names != set(section["measured"]) or names != set(section["margin"]):
            raise SystemExit(f"{section_name} receipt fields do not match")
        for name in sorted(names):
            if section["measured"][name] + section["margin"][name] != section["accepted"][name]:
                raise SystemExit(f"{section_name}.{name}: receipt arithmetic does not add up")
    for name, values in receipt["planner_process"].items():
        if values["measured"] + values["margin"] != values["accepted"]:
            raise SystemExit(f"planner_process.{name}: receipt arithmetic does not add up")
    cancellation = receipt["cancellation"]
    for measured, margin, accepted in [
        (
            "measured_acknowledgement_millis",
            "margin_millis",
            "accepted_acknowledgement_millis",
        ),
        (
            "measured_final_process_cleanup_millis",
            "final_process_cleanup_margin_millis",
            "accepted_final_process_cleanup_millis",
        ),
        (
            "measured_host_cancellation_completion_millis",
            "host_cancellation_completion_margin_millis",
            "accepted_host_cancellation_completion_millis",
        ),
    ]:
        if cancellation[measured] + cancellation[margin] != cancellation[accepted]:
            raise SystemExit(f"cancellation.{accepted}: receipt arithmetic does not add up")


def compare_measurements(receipt: dict, measured: dict, process: dict) -> None:
    checked = receipt["planning"]["measured"]
    receipt_fields = set(checked) - {"worker_wall_millis"}
    if receipt_fields != DETERMINISTIC_FIELDS | UNMEASURED_FIELDS:
        raise SystemExit("planning receipt measurement fields do not match the verifier")
    for name in sorted(DETERMINISTIC_FIELDS):
        if measured[name] != checked[name]:
            raise SystemExit(
                f"planning.{name}: measured {measured[name]} != checked receipt {checked[name]}"
            )
    wall = measured["worker_wall_millis"]
    wall_baseline = checked["worker_wall_millis"]
    wall_limit = receipt["planning"]["accepted"]["worker_wall_millis"]
    wall_floor = receipt["verification"]["minimum_worker_wall_margin_millis"]
    if wall + wall_floor > wall_limit:
        raise SystemExit(
            "planning.worker_wall_millis lost safety margin: "
            f"measured {wall} + required margin {wall_floor} > limit {wall_limit}"
        )
    if wall > wall_baseline * 2:
        raise SystemExit(
            "planning.worker_wall_millis regressed beyond twice its checked baseline: "
            f"measured {wall}, baseline {wall_baseline}"
        )
    for name, actual in sorted(process.items()):
        checked_value = receipt["planner_process"][name]["measured"]
        if name == "address_space_bytes":
            accepted = receipt["planner_process"][name]["accepted"]
            minimum = receipt["verification"]["minimum_address_space_margin_bytes"]
            if actual + minimum > accepted:
                raise SystemExit(
                    f"planner_process.{name} lost safety margin: measured {actual}, "
                    f"required margin {minimum}, limit {accepted}"
                )
            if actual > checked_value * 2:
                raise SystemExit(
                    f"planner_process.{name}: measured {actual} exceeds twice the "
                    f"checked baseline {checked_value}"
                )
        elif actual != checked_value:
            raise SystemExit(
                f"planner_process.{name}: measured {actual} != checked receipt {checked_value}"
            )


def verify_rust_constants(receipt: dict) -> None:
    source = WORKFLOW_CLI_PATH.read_text()
    names = {
        "PLANNER_REQUEST_FRAME_BYTES": "request_frame_bytes",
        "PLANNER_RESPONSE_FRAME_BYTES": "response_frame_bytes",
        "PLANNER_STDERR_BYTES": "stderr_bytes",
        "PLANNER_ADDRESS_SPACE_BYTES": "address_space_bytes",
    }
    for rust_name, receipt_name in names.items():
        actual = integer_expression(source, rust_name)
        accepted = receipt["planner_process"][receipt_name]["accepted"]
        if actual != accepted:
            raise SystemExit(f"{rust_name}: Rust value {actual} != receipt {accepted}")
    model = WORKFLOW_MODEL_PATH.read_text()
    condition_depth_limit = integer_expression_from(
        model, "CONDITION_DEPTH_LIMIT", WORKFLOW_MODEL_PATH
    )
    accepted_depth = receipt["planning"]["accepted"]["condition_depth"]
    if condition_depth_limit != accepted_depth:
        raise SystemExit(
            f"CONDITION_DEPTH_LIMIT: Rust value {condition_depth_limit} != receipt {accepted_depth}"
        )
    exact_process = WORKFLOW_EXACT_PROCESS_PATH.read_text()
    for rust_name, receipt_name in [
        ("FINAL_PROCESS_CLEANUP_MILLIS", "accepted_final_process_cleanup_millis"),
        ("HOST_CANCELLATION_COMPLETION_MILLIS", "accepted_host_cancellation_completion_millis"),
    ]:
        actual = integer_expression_from(exact_process, rust_name, WORKFLOW_EXACT_PROCESS_PATH)
        accepted = receipt["cancellation"][receipt_name]
        if actual != accepted:
            raise SystemExit(f"{rust_name}: Rust value {actual} != receipt {accepted}")
    agent_cleanup = integer_expression_from(
        WORKFLOW_CANCELLATION_PATH.read_text(),
        "AGENT_CANCELLATION_CLEANUP_MILLIS",
        WORKFLOW_CANCELLATION_PATH,
    )
    accepted_agent_cleanup = receipt["cancellation"][
        "accepted_host_cancellation_completion_millis"
    ]
    if agent_cleanup != accepted_agent_cleanup:
        raise SystemExit(
            "AGENT_CANCELLATION_CLEANUP_MILLIS: "
            f"Rust value {agent_cleanup} != receipt {accepted_agent_cleanup}"
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rho", type=Path, required=True, help="rho binary to measure")
    parser.add_argument(
        "--skip-public-validation",
        action="store_true",
        help="developer-only fast loop; receipt verification still uses the planner worker",
    )
    parser.add_argument("--json-output", type=Path, help="write all per-case measurements")
    args = parser.parse_args()

    receipt = load_receipt()
    verify_arithmetic(receipt)
    verify_rust_constants(receipt)
    measured, process, cases = measure_corpus(
        args.rho.resolve(), public_validation=not args.skip_public_validation
    )
    compare_measurements(receipt, measured, process)
    if args.json_output:
        args.json_output.write_text(
            json.dumps(
                {"planning": measured, "planner_process": process, "cases": cases},
                indent=2,
                sort_keys=True,
            )
            + "\n"
        )
    print("workflow limit receipt verified against the generated acceptance corpus")


if __name__ == "__main__":
    main()
