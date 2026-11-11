"""Measure an MCP stdio server's startup, tool count and idle process-tree RSS.

The same harness can compare the Python server with a future Go/Rust binary:

    python scripts/benchmark_server.py --label http \
      --env ECLASS_BROWSER=http --env ECLASS_SESSION_FILE=/path/session.json

    python scripts/benchmark_server.py --label go --command './eclass-mcp serve'

The idle measurement keeps stdin open so the server stays in its lifespan.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import signal
import statistics
import subprocess
import sys
import time
from pathlib import Path


def process_tree(pid: int) -> tuple[float, int] | None:
    try:
        output = subprocess.check_output(
            ["ps", "-axo", "pid=,ppid=,rss="], text=True, stderr=subprocess.DEVNULL
        )
    except (OSError, subprocess.CalledProcessError):
        return None
    rows = []
    for line in output.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[0].isdigit() and parts[1].isdigit() and parts[2].isdigit():
            rows.append((int(parts[0]), int(parts[1]), int(parts[2])))
    found, changed = {pid}, True
    while changed:
        changed = False
        for child, parent, _ in rows:
            if parent in found and child not in found:
                found.add(child)
                changed = True
    rss = sum(value for child, _, value in rows if child in found) * 1024
    return rss / (1024 * 1024), len(found)


def idle(command: list[str], cwd: Path, env: dict, seconds: float) -> tuple[float, int] | None:
    kwargs = {
        "cwd": cwd,
        "env": env,
        "stdin": subprocess.PIPE,
        "stdout": subprocess.DEVNULL,
        "stderr": subprocess.DEVNULL,
    }
    if os.name == "nt":
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kwargs["start_new_session"] = True
    process = subprocess.Popen(command, **kwargs)
    try:
        time.sleep(seconds)
        return process_tree(process.pid)
    finally:
        if os.name == "nt":
            process.terminate()
        else:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            if os.name == "nt":
                process.kill()
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    process.kill()
            process.wait()


async def startup(command: list[str], cwd: Path, env: dict) -> tuple[float, int]:
    from fastmcp import Client
    from fastmcp.client.transports import StdioTransport

    started = time.perf_counter()
    async with Client(
        StdioTransport(command=command[0], args=command[1:], env=env, cwd=str(cwd)),
        timeout=60,
    ) as client:
        tools = await client.list_tools()
    return time.perf_counter() - started, len(tools)


def parse_env(values: list[str]) -> dict:
    env = {**os.environ}
    for value in values:
        key, separator, item = value.partition("=")
        if not separator or not key:
            raise SystemExit(f"--env must be KEY=VALUE, got: {value}")
        env[key] = item
    return env


async def main_async(args):
    cwd = Path(args.cwd).expanduser().resolve()
    env = parse_env(args.env)
    command = args.command if isinstance(args.command, list) else [args.command]
    startup_values, rss_values, process_counts, tool_count = [], [], [], None
    for _ in range(args.runs):
        seconds, tool_count = await startup(command, cwd, env)
        startup_values.append(seconds)
        measured = idle(command, cwd, env, args.idle_seconds)
        if measured is not None:
            rss, count = measured
            rss_values.append(rss)
            process_counts.append(count)
        time.sleep(0.3)
    result = {
        "label": args.label,
        "command": command,
        "cwd": str(cwd),
        "runs": args.runs,
        "tools": tool_count,
        "startup_seconds_median": round(statistics.median(startup_values), 3),
        "startup_seconds_min": round(min(startup_values), 3),
        "startup_seconds_max": round(max(startup_values), 3),
    }
    if rss_values:
        result.update(
            idle_rss_mb_median=round(statistics.median(rss_values), 1),
            idle_processes_median=int(statistics.median(process_counts)),
        )
    else:
        result["idle_rss_mb_median"] = None
        result["idle_processes_median"] = None
    print(json.dumps(result, ensure_ascii=False, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", default="eclass-mcp")
    parser.add_argument(
        "--command",
        nargs="+",
        default=[sys.executable, "-m", "eclass_mcp.cli", "serve"],
        help="Server command; quote paths with spaces",
    )
    parser.add_argument("--cwd", default=".", help="Working directory")
    parser.add_argument("--env", action="append", default=[], help="KEY=VALUE (repeatable)")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--idle-seconds", type=float, default=4)
    args = parser.parse_args()
    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()
