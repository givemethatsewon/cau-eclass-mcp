"""Human-facing commands; only `serve` writes MCP protocol to stdout."""

from __future__ import annotations

import argparse
import asyncio
import importlib.metadata
import importlib.resources
import json
import os
import shutil
import sys
from pathlib import Path

from .config import create_transport
from .service import EClassService
from .transport import EClassError


def version():
    return importlib.metadata.version("cau-eclass-mcp")


def generated_config(client: str):
    env = {"ECLASS_BROWSER": os.getenv("ECLASS_BROWSER", "aside"), "ECLASS_ENABLE_SUBMISSION": "0"}
    for key in ("ECLASS_CDP_ENDPOINT", "ECLASS_ASIDE_BIN", "ECLASS_ASIDE_PROFILE", "ECLASS_DOWNLOAD_DIR"):
        if value := os.getenv(key):
            env[key] = value
    command = str(Path(sys.executable).absolute())
    args = ["-m", "eclass_mcp.cli", "serve"]
    if client == "codex":
        # JSON strings/arrays are valid TOML strings/arrays for these values.
        return "\n".join(
            [
                "[mcp_servers.eclass]",
                f"command = {json.dumps(command)}",
                f"args = {json.dumps(args)}",
                "",
                "[mcp_servers.eclass.env]",
                *(f"{k} = {json.dumps(v)}" for k, v in env.items()),
            ]
        )
    entry = {"command": command, "args": args, "env": env}
    if client == "vscode":
        entry["type"] = "stdio"
    return json.dumps({"servers" if client == "vscode" else "mcpServers": {"eclass": entry}}, indent=2)


async def doctor():
    checks = {
        "version": version(),
        "python": sys.version.split()[0],
        "browser": os.getenv("ECLASS_BROWSER", "aside"),
        "submission_enabled": os.getenv("ECLASS_ENABLE_SUBMISSION") == "1",
    }
    try:
        if checks["browser"] == "aside" and not shutil.which(os.getenv("ECLASS_ASIDE_BIN", "aside")):
            raise EClassError("Aside CLI not found; set ECLASS_ASIDE_BIN or choose --browser cdp")
        async with create_transport() as transport:
            # Only profile GET, no course material, uploads or submission.
            status = await EClassService(transport).session_status()
            checks["authenticated"] = status.get("authenticated", False)
        checks["ok"] = bool(checks["authenticated"])
    except EClassError as error:
        checks.update(ok=False, error=str(error))
    except Exception:
        checks.update(ok=False, error="Startup check failed; verify browser connection and login")
    print(json.dumps(checks, ensure_ascii=False, indent=2))
    return 0 if checks["ok"] else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description="CAU e-Class MCP: use an existing logged-in browser")
    parser.add_argument("--version", action="version", version=version())
    parser.add_argument("--browser", choices=("aside", "cdp"), help="Browser adapter (env: ECLASS_BROWSER)")
    parser.add_argument(
        "--cdp-endpoint", help="Local Chromium CDP URL; selects cdp unless --browser is explicit"
    )
    parser.add_argument("--download-dir", type=Path, help="Absolute directory for downloaded files")
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("serve", help="Run the stdio MCP server (default)")
    commands.add_parser("doctor", help="Check connectivity and login without displaying account data")
    config = commands.add_parser("config", help="Print configuration; never modify client settings")
    config.add_argument("--client", choices=("claude", "cursor", "vscode", "codex"), default="claude")
    commands.add_parser("skill", help="Print the bundled agent SKILL.md")
    args = parser.parse_args(argv)
    if args.command in {"doctor", "config", "skill"} and hasattr(sys.stdout, "reconfigure"):
        # Redirected Windows output otherwise uses a legacy code page.
        sys.stdout.reconfigure(encoding="utf-8")
    if args.browser:
        os.environ["ECLASS_BROWSER"] = args.browser
    if args.cdp_endpoint:
        if args.browser == "aside":
            parser.error("--cdp-endpoint cannot be used with --browser aside")
        from .cdp import validate_endpoint

        try:
            os.environ["ECLASS_CDP_ENDPOINT"] = validate_endpoint(args.cdp_endpoint)
        except EClassError as error:
            parser.error(str(error))
        os.environ["ECLASS_BROWSER"] = "cdp"
    if args.download_dir:
        os.environ["ECLASS_DOWNLOAD_DIR"] = str(args.download_dir.expanduser().resolve())
    if args.command == "doctor":
        return asyncio.run(doctor())
    if args.command == "config":
        print(generated_config(args.client))
        return 0
    if args.command == "skill":
        resource = importlib.resources.files("eclass_mcp").joinpath("SKILL.md")
        if not resource.is_file():  # Editable source checkout; wheels contain the resource.
            resource = Path(__file__).resolve().parents[2] / "skills/cau-eclass/SKILL.md"
        print(resource.read_text(encoding="utf-8"))
        return 0
    from .server import main as serve

    try:
        serve()
    except EClassError as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
