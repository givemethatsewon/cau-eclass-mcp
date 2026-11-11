"""Human-facing commands; only `serve` writes MCP protocol to stdout."""

from __future__ import annotations

import argparse
import asyncio
import importlib.metadata
import importlib.resources
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from .config import create_transport, resolve_backend
from .http_transport import HTTPTransport
from .service import EClassService
from .session import (
    default_session_file,
    delete_session,
    load_session,
    login_via_cdp,
    refresh_session,
    save_session,
)
from .transport import EClassError


def version():
    return importlib.metadata.version("cau-eclass-mcp")


def generated_config(client: str):
    backend = os.getenv("ECLASS_BROWSER") or resolve_backend()
    env = {"ECLASS_BROWSER": backend, "ECLASS_ENABLE_SUBMISSION": "0"}
    for key in (
        "ECLASS_CDP_ENDPOINT",
        "ECLASS_ASIDE_BIN",
        "ECLASS_ASIDE_PROFILE",
        "ECLASS_DOWNLOAD_DIR",
        "ECLASS_SESSION_FILE",
        "ECLASS_FALLBACK_BROWSER",
        "ECLASS_CACHE_TTL",
        "ECLASS_AUTO_REFRESH",
    ):
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


async def _probe(checks: dict):
    async with create_transport() as transport:
        status = await EClassService(transport).session_status()
        checks.update(
            authenticated=status.get("authenticated", False),
            transport=status.get("transport"),
        )
    return checks


async def doctor():
    backend = resolve_backend()
    checks = {
        "version": version(),
        "python": sys.version.split()[0],
        "browser": backend,
        "submission_enabled": os.getenv("ECLASS_ENABLE_SUBMISSION") == "1",
    }
    if backend == "http":
        path = default_session_file()
        checks["session_file"] = str(path)
        checks["session_present"] = path.exists()
        if path.exists():
            try:
                checks["session_saved_at"] = load_session(path).saved_at
            except EClassError:
                checks["session_present"] = False
    if backend == "aside" and not shutil.which(os.getenv("ECLASS_ASIDE_BIN", "aside")):
        checks.update(ok=False, error="Aside CLI not found; set ECLASS_ASIDE_BIN or choose --browser http")
        print(json.dumps(checks, ensure_ascii=False, indent=2))
        return 1
    try:
        await _probe(checks)
        checks["ok"] = bool(checks.get("authenticated"))
    except EClassError as error:
        checks.update(ok=False, error=str(error))
        if backend == "http":
            try:
                refreshed = await refresh_session()
                async with HTTPTransport(session=refreshed) as transport:
                    status = await EClassService(transport).session_status()
                checks.update(
                    ok=bool(status.get("authenticated")),
                    authenticated=status.get("authenticated", False),
                    refreshed=True,
                    session_present=True,
                    session_saved_at=refreshed.saved_at,
                )
                checks.pop("error", None)
            except EClassError:
                pass
    except Exception:
        checks.update(ok=False, error="Startup check failed; verify the saved session and login")
    print(json.dumps(checks, ensure_ascii=False, indent=2))
    return 0 if checks.get("ok") else 1


def find_browser(explicit: str | None = None) -> Path:
    if explicit:
        path = Path(explicit).expanduser()
        if not path.is_file():
            raise EClassError("The browser executable path does not exist")
        return path
    configured = os.getenv("ECLASS_BROWSER_PATH")
    if configured:
        path = Path(configured).expanduser()
        if path.is_file():
            return path
    candidates = []
    if sys.platform == "darwin":
        for app in ("Google Chrome", "Microsoft Edge", "Brave Browser", "Chromium"):
            candidates.append(Path("/Applications") / f"{app}.app/Contents/MacOS/{app}")
    elif os.name == "nt":
        for root in (os.getenv("ProgramFiles"), os.getenv("ProgramFiles(x86)")):
            if root:
                candidates.extend(
                    [
                        Path(root) / "Google/Chrome/Application/chrome.exe",
                        Path(root) / "Microsoft/Edge/Application/msedge.exe",
                        Path(root) / "BraveSoftware/Brave-Browser/Application/brave.exe",
                    ]
                )
    for name in (
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "brave-browser",
    ):
        if found := shutil.which(name):
            candidates.append(Path(found))
    for path in candidates:
        if path.is_file():
            return path
    raise EClassError(
        "Chrome, Edge or Chromium was not found; pass --browser-path /absolute/path/to/browser"
    )


def launch_browser(executable: Path, profile_dir: Path, port: int, url: str) -> None:
    profile_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    args = [
        str(executable),
        "--remote-debugging-address=127.0.0.1",
        f"--remote-debugging-port={port}",
        f"--user-data-dir={profile_dir}",
        "--no-first-run",
        "--no-default-browser-check",
        url,
    ]
    kwargs = {
        "stdout": subprocess.DEVNULL,
        "stderr": subprocess.DEVNULL,
        "stdin": subprocess.DEVNULL,
    }
    if os.name == "nt":
        kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kwargs["start_new_session"] = True
    subprocess.Popen(args, **kwargs)


async def login(args):
    endpoint = args.login_cdp_endpoint or args.cdp_endpoint or f"http://127.0.0.1:{args.port}"
    if not args.no_launch:
        executable = find_browser(args.browser_path)
        profile = Path(args.profile_dir).expanduser().resolve()
        launch_browser(executable, profile, args.port, "https://eclass3.cau.ac.kr/")
        print(
            "A dedicated browser window is open. Sign in to e-Class; this command waits "
            f"up to {args.timeout} seconds.",
            file=sys.stderr,
            flush=True,
        )
    else:
        print(
            f"Reading the existing Chromium debugging session at {endpoint}.",
            file=sys.stderr,
            flush=True,
        )
    session = await login_via_cdp(
        endpoint, timeout=args.timeout, wait_for_browser=not args.no_launch
    )
    path = save_session(
        session, Path(args.session_file).expanduser() if args.session_file else None
    )
    print(
        json.dumps(
            {
                "ok": True,
                "authenticated": True,
                "session_file": str(path),
                "saved_at": session.saved_at,
                "learningx_token": bool(session.token("xn_api_token")),
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


def logout(args):
    path = Path(args.session_file).expanduser() if args.session_file else default_session_file()
    removed = delete_session(path)
    print(json.dumps({"removed": removed, "session_file": str(path)}, ensure_ascii=False, indent=2))
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description="CAU e-Class MCP: reuse a logged-in browser over HTTP")
    parser.add_argument("--version", action="version", version=version())
    parser.add_argument(
        "--browser", choices=("http", "aside", "cdp"), help="Transport (env: ECLASS_BROWSER)"
    )
    parser.add_argument(
        "--cdp-endpoint", help="Local Chromium CDP URL for login or the legacy cdp adapter"
    )
    parser.add_argument("--download-dir", type=Path, help="Absolute directory for downloaded files")
    parser.add_argument("--session-file", type=Path, help="Saved session path (env: ECLASS_SESSION_FILE)")
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("serve", help="Run the stdio MCP server (default)")
    commands.add_parser("doctor", help="Check connectivity and login without displaying account data")
    login_parser = commands.add_parser("login", help="Open a browser once and save its session")
    login_parser.add_argument("--browser-path", help="Chrome/Edge/Chromium executable")
    login_parser.add_argument(
        "--cdp-endpoint",
        dest="login_cdp_endpoint",
        help="Existing Chromium CDP URL (with --no-launch)",
    )
    login_parser.add_argument(
        "--profile-dir",
        default=str(Path.home() / ".cau-eclass" / "chrome-profile"),
        help="Dedicated browser profile directory",
    )
    login_parser.add_argument("--port", type=int, default=9222, help="Local CDP port")
    login_parser.add_argument(
        "--timeout", type=float, default=300, help="Seconds to wait for the sign-in"
    )
    login_parser.add_argument(
        "--no-launch", action="store_true", help="Read an already running CDP browser instead"
    )
    commands.add_parser("logout", help="Delete the saved session")
    config = commands.add_parser("config", help="Print configuration; never modify client settings")
    config.add_argument("--client", choices=("claude", "cursor", "vscode", "codex"), default="claude")
    commands.add_parser("skill", help="Print the bundled agent SKILL.md")
    args = parser.parse_args(argv)
    if args.command in {"doctor", "config", "skill", "login", "logout"} and hasattr(sys.stdout, "reconfigure"):
        # Redirected Windows output otherwise uses a legacy code page.
        sys.stdout.reconfigure(encoding="utf-8")
    if args.browser:
        os.environ["ECLASS_BROWSER"] = args.browser
    if args.session_file:
        os.environ["ECLASS_SESSION_FILE"] = str(args.session_file.expanduser().resolve())
    if args.cdp_endpoint:
        from .cdp import validate_endpoint

        try:
            os.environ["ECLASS_CDP_ENDPOINT"] = validate_endpoint(args.cdp_endpoint)
        except EClassError as error:
            parser.error(str(error))
        if args.command != "login" and args.browser != "aside":
            os.environ["ECLASS_BROWSER"] = "cdp"
    if args.download_dir:
        os.environ["ECLASS_DOWNLOAD_DIR"] = str(args.download_dir.expanduser().resolve())
    if args.command == "login":
        try:
            return asyncio.run(login(args))
        except EClassError as error:
            print(str(error), file=sys.stderr)
            return 1
    if args.command == "logout":
        return logout(args)
    if args.command == "doctor":
        return asyncio.run(doctor())
    if args.command == "config":
        if resolve_backend() == "http" and not default_session_file().exists():
            print(
                "No saved session; run `eclass-mcp login` before starting the MCP client.",
                file=sys.stderr,
            )
        print(generated_config(args.client))
        return 0
    if args.command == "skill":
        resource = importlib.resources.files("eclass_mcp").joinpath("SKILL.md")
        if not resource.is_file():  # Editable source checkout; wheels contain the resource.
            resource = Path(__file__).resolve().parents[2] / "skills/cau-eclass/SKILL.md"
        print(resource.read_text(encoding="utf-8"), end="")
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
