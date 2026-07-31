#!/usr/bin/env python3
"""Measure active workflow cancellation across three OS processes."""

from __future__ import annotations

import argparse
import json
import os
import select
import socket
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RECEIPT = ROOT / "crates/rho/src/workflow/fixtures/limit_receipt.json"
WAIT_SOURCE = ROOT / "crates/rho/src/workflow/fixtures/cancellation_wait.rs"


def elapsed_millis(started: int) -> int:
    return (time.monotonic_ns() - started + 999_999) // 1_000_000


def wait_for_pid_exit(descriptor: int, pid: int, timeout_millis: int) -> None:
    poller = select.poll()
    poller.register(descriptor, select.POLLIN)
    if not poller.poll(timeout_millis):
        raise SystemExit(
            f"active command process {pid} survived the {timeout_millis} ms cleanup limit"
        )


def wait_for_ready(server: socket.socket, owner: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 10
    server.settimeout(0.1)
    while time.monotonic() < deadline:
        try:
            connection, _ = server.accept()
        except TimeoutError:
            connection = None
        if connection is not None:
            with connection:
                if connection.recv(1) == b"x":
                    return
        if owner.poll() is not None:
            stdout, stderr = owner.communicate()
            raise SystemExit(
                "workflow owner exited before its command became active:\n"
                f"stdout:\n{stdout}\nstderr:\n{stderr}"
            )
    raise SystemExit("workflow command did not become active within 10 seconds")


def fixture_source() -> str:
    return """\
def build(inputs):
    return workflow(name = "cancellation_receipt", nodes = [command(
        name = "active",
        argv = ["./cancellation-wait", "ready.sock", "process.pid"],
        cwd = ".",
        timeout_seconds = 86400,
        max_output_bytes = 1,
    )])
WORKFLOW = define(inputs = {}, build = build)
"""


def measure_once(rho: Path, root: Path, limits: dict) -> dict[str, int]:
    root.mkdir()
    home = root / "home"
    workspace = root / "workspace"
    home.mkdir()
    workspace.mkdir()
    ready_socket = workspace / "ready.sock"
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(ready_socket))
    server.listen(1)
    (workspace / "workflow.star").write_text(fixture_source())
    executable = workspace / "cancellation-wait"
    compiled = subprocess.run(
        ["rustc", "--edition=2021", str(WAIT_SOURCE), "-o", str(executable)],
        capture_output=True,
        text=True,
        check=False,
    )
    if compiled.returncode != 0:
        raise SystemExit(f"cancellation helper compilation failed:\n{compiled.stderr}")
    env = {**os.environ, "RHO_HOME": str(home)}
    planned = subprocess.run(
        [str(rho), "workflow", "plan", "workflow.star", "--output", "json"],
        cwd=workspace,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if planned.returncode != 0:
        raise SystemExit(f"cancellation fixture plan failed:\n{planned.stderr}")
    plan_id = json.loads(planned.stdout)["manifest"]["plan_id"]
    owner = subprocess.Popen(
        [str(rho), "workflow", "run", plan_id, "--yes", "--output", "jsonl"],
        cwd=workspace,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        wait_for_ready(server, owner)
        run_ids = [path.name for path in (home / "workflows/runs").iterdir() if path.is_dir()]
        if len(run_ids) != 1:
            raise SystemExit(f"expected one active run, found {run_ids}")
        command_pid = int((workspace / "process.pid").read_text())
        try:
            pid_descriptor = os.pidfd_open(command_pid)
        except AttributeError as error:
            raise SystemExit("cancellation measurement needs Linux pidfd_open") from error

        try:
            started = time.monotonic_ns()
            cancelled = subprocess.run(
                [str(rho), "workflow", "cancel", run_ids[0]],
                cwd=workspace,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            acknowledgement = elapsed_millis(started)
            if cancelled.returncode != 0:
                raise SystemExit(f"cross-process cancel failed:\n{cancelled.stderr}")
            result = json.loads(cancelled.stdout)
            if result["cancellation_state"] != "acknowledged":
                raise SystemExit(f"cross-process cancel was not acknowledged: {result}")

            wait_for_pid_exit(
                pid_descriptor,
                command_pid,
                limits["accepted_final_process_cleanup_millis"],
            )
        finally:
            os.close(pid_descriptor)
        cleanup = elapsed_millis(started)
        try:
            owner_stdout, owner_stderr = owner.communicate(
                timeout=limits["accepted_host_cancellation_completion_millis"] / 1000
            )
        except subprocess.TimeoutExpired as error:
            raise SystemExit("active workflow owner did not finish after cancellation") from error
        host_completion = elapsed_millis(started)
        if owner.returncode != 0:
            raise SystemExit(
                "active workflow owner failed after cancellation:\n"
                f"stdout:\n{owner_stdout}\nstderr:\n{owner_stderr}"
            )
        return {
            "acknowledgement_millis": acknowledgement,
            "final_process_cleanup_millis": cleanup,
            "host_cancellation_completion_millis": host_completion,
        }
    finally:
        server.close()
        if owner.poll() is None:
            owner.terminate()
            try:
                owner.wait(timeout=2)
            except subprocess.TimeoutExpired:
                owner.kill()
                owner.wait()


def verify(receipt: dict, measured: dict[str, int]) -> None:
    cancellation = receipt["cancellation"]
    mappings = {
        "acknowledgement_millis": (
            "measured_acknowledgement_millis",
            "accepted_acknowledgement_millis",
        ),
        "final_process_cleanup_millis": (
            "measured_final_process_cleanup_millis",
            "accepted_final_process_cleanup_millis",
        ),
        "host_cancellation_completion_millis": (
            "measured_host_cancellation_completion_millis",
            "accepted_host_cancellation_completion_millis",
        ),
    }
    for name, (baseline_name, limit_name) in mappings.items():
        actual = measured[name]
        baseline = cancellation[baseline_name]
        limit = cancellation[limit_name]
        if actual > limit:
            raise SystemExit(f"cancellation {name}: measured {actual} ms exceeds limit {limit} ms")
        if baseline and actual > baseline * 2:
            raise SystemExit(
                f"cancellation {name}: measured {actual} ms exceeds twice the "
                f"checked baseline {baseline} ms"
            )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rho", type=Path, required=True)
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args()
    if args.repeat < 1:
        raise SystemExit("--repeat must be at least 1")
    receipt = json.loads(RECEIPT.read_text())
    limits = receipt["cancellation"]
    samples = []
    with tempfile.TemporaryDirectory(prefix="rho-workflow-cancel-") as directory:
        root = Path(directory)
        for index in range(args.repeat):
            sample = measure_once(args.rho.resolve(), root / str(index), limits)
            samples.append(sample)
            print(f"cancellation sample {index + 1}: {sample}")
    maxima = {name: max(sample[name] for sample in samples) for name in samples[0]}
    verify(receipt, maxima)
    if args.json_output:
        args.json_output.write_text(
            json.dumps({"maxima": maxima, "samples": samples}, indent=2) + "\n"
        )
    print(f"cross-process cancellation receipt verified: {maxima}")


if __name__ == "__main__":
    main()
