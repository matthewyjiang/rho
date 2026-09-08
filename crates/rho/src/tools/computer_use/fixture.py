#!/usr/bin/env python3
"""Stdio-only fake Cua Driver. It never observes or controls a desktop."""
import json
import sys
import os
import socket

assert sys.argv[1:] == ["mcp"]

if os.path.exists("blocked-connect.sock"):
    signal = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    signal.connect("blocked-connect.sock")
    signal.recv(1)
    sys.exit(0)


def send(message):
    print(json.dumps(message), flush=True)


for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fake-cua", "version": "1"},
            "instructions": "Observe before acting.",
        }
    elif method == "tools/list":
        result = {"tools": [
            {"name": name, "description": name, "inputSchema": {"type": "object"}}
            for name in ["click", "get_window_state", "set_config", "start_recording", "future_tool"]
        ]}
    elif method == "tools/call":
        params = message["params"]
        assert params["name"] in ["click", "get_window_state"]
        assert "session" not in params["arguments"]
        if params["name"] == "click":
            token = params.get("_meta", {}).get("progressToken")
            if token is not None:
                send({"jsonrpc": "2.0", "method": "notifications/progress", "params": {
                    "progressToken": token, "progress": 1, "message": "action received"
                }})
            continue
        result = {"content": [
            {"type": "text", "text": "observed"},
            {"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII="},
        ], "isError": False}
    else:
        result = {}
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})
