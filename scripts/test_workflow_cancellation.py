#!/usr/bin/env python3
"""Acceptance test for active workflow cancellation across three OS processes.

The command signals readiness over a Unix socket before cancellation begins.
Deadlines bound acknowledgement, child cleanup, and owner completion; no timing
samples, baselines, or result files are recorded. Requires Linux pidfd_open and
rustc on PATH, and accepts an existing rho binary via --rho.
"""

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
WAIT_SOURCE = ROOT / "crates/rho/src/workflow/fixtures/cancellation_wait.rs"
ACKNOWLEDGEMENT_MILLIS = 2_000
FINAL_PROCESS_CLEANUP_MILLIS = 2_000
HOST_CANCELLATION_COMPLETION_MILLIS = 2_500


def wait_for_pid_exit(descriptor: int, pid: int, deadline: float) -> None:
    poller = select.poll()
    poller.register(descriptor, select.POLLIN)
    remaining_millis = max(0, int((deadline - time.monotonic()) * 1_000))
    if not poller.poll(remaining_millis):
        raise SystemExit(
            f"active command process {pid} survived the "
            f"{FINAL_PROCESS_CLEANUP_MILLIS} ms cleanup limit"
        )


def wait_for_ready(server: socket.socket, owner: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 10
    server.settimeout(0.1)
    while time.monotonic() < deadline:
        try:
            connection, _ = server.accept()
        except (TimeoutError, socket.timeout):
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
    return workflow(name = "cancellation_acceptance", nodes = [command(
        name = "active",
        argv = ["./cancellation-wait", "ready.sock", "process.pid"],
        cwd = ".",
        timeout_seconds = 86400,
        max_output_bytes = 1,
    )])
WORKFLOW = define(inputs = {}, build = build)
"""


def check_cancellation(rho: Path, root: Path) -> None:
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
    # Agent catalogs also read HOME; RHO_HOME alone does not isolate user agents.
    env = {**os.environ, "HOME": str(home), "RHO_HOME": str(home)}
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
        inventory = subprocess.run(
            [str(rho), "workflow", "list", "--runs", "--json"],
            cwd=workspace,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        if inventory.returncode != 0:
            raise SystemExit(f"cancellation fixture inventory failed:\n{inventory.stderr}")
        run_ids = [run["id"] for run in json.loads(inventory.stdout)["runs"]]
        if len(run_ids) != 1:
            raise SystemExit(f"expected one active run, found {run_ids}")
        command_pid = int((workspace / "process.pid").read_text())
        try:
            pid_descriptor = os.pidfd_open(command_pid)
        except AttributeError as error:
            raise SystemExit("cancellation acceptance test needs Linux pidfd_open") from error

        try:
            started = time.monotonic()
            try:
                cancelled = subprocess.run(
                    [str(rho), "workflow", "cancel", run_ids[0]],
                    cwd=workspace,
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                    timeout=ACKNOWLEDGEMENT_MILLIS / 1_000,
                )
            except subprocess.TimeoutExpired as error:
                raise SystemExit(
                    f"cross-process cancel exceeded the {ACKNOWLEDGEMENT_MILLIS} ms "
                    "acknowledgement limit"
                ) from error
            if cancelled.returncode != 0:
                raise SystemExit(f"cross-process cancel failed:\n{cancelled.stderr}")
            result = json.loads(cancelled.stdout)
            if result["cancellation_state"] != "acknowledged":
                raise SystemExit(f"cross-process cancel was not acknowledged: {result}")

            wait_for_pid_exit(
                pid_descriptor,
                command_pid,
                started + FINAL_PROCESS_CLEANUP_MILLIS / 1_000,
            )
        finally:
            os.close(pid_descriptor)
        try:
            owner_stdout, owner_stderr = owner.communicate(
                timeout=max(
                    0,
                    started + HOST_CANCELLATION_COMPLETION_MILLIS / 1_000 - time.monotonic(),
                )
            )
        except subprocess.TimeoutExpired as error:
            raise SystemExit("active workflow owner did not finish after cancellation") from error
        if owner.returncode != 0:
            raise SystemExit(
                "active workflow owner failed after cancellation:\n"
                f"stdout:\n{owner_stdout}\nstderr:\n{owner_stderr}"
            )
    finally:
        server.close()
        if owner.poll() is None:
            owner.terminate()
            try:
                owner.wait(timeout=2)
            except subprocess.TimeoutExpired:
                owner.kill()
                owner.wait()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rho", type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="rho-workflow-cancel-") as directory:
        check_cancellation(args.rho.resolve(), Path(directory))
    print("cross-process workflow cancellation acceptance test passed")


if __name__ == "__main__":
    main()
