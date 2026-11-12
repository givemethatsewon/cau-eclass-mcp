"""Read the owner-only session file written by the Rust login command.

This module intentionally does not sign in, refresh cookies, or make e-Class
requests. The Rust MCP owns authentication; the Python helper only needs the
`.cau.ac.kr` cookies to render one OZ syllabus page.
"""

from __future__ import annotations

import json
import os
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

from .errors import EClassError

COOKIE_DOMAIN = "cau.ac.kr"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def default_session_file() -> Path:
    override = os.getenv("ECLASS_SESSION_FILE")
    if override:
        return Path(override).expanduser()
    if sys.platform == "darwin":
        base = Path.home() / "Library" / "Application Support" / "cau-eclass"
    elif os.name == "nt":
        base = Path(os.getenv("APPDATA", str(Path.home()))) / "cau-eclass"
    else:
        base = Path(os.getenv("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "cau-eclass"
    return base / "session.json"


def _cookie_domain(value) -> str:
    return str(value or "").lstrip(".").lower()


def _is_cau_domain(value) -> bool:
    domain = _cookie_domain(value)
    return domain == COOKIE_DOMAIN or domain.endswith("." + COOKIE_DOMAIN)


def _compact_cookie(value: dict) -> dict:
    keys = ("name", "value", "domain", "path", "expires", "secure", "httpOnly", "sameSite", "hostOnly")
    return {key: value[key] for key in keys if key in value}


@dataclass
class Session:
    cookies: list[dict] = field(default_factory=list)
    saved_at: str = field(default_factory=now_iso)
    version: int = 1
    browser: str | None = None

    @classmethod
    def from_json(cls, value: dict) -> "Session":
        if not isinstance(value, dict) or value.get("version") != 1:
            raise EClassError("Unsupported session file format")
        cookies = value.get("cookies")
        if not isinstance(cookies, list) or not all(
            isinstance(cookie, dict) and cookie.get("name") and cookie.get("value") is not None
            for cookie in cookies
        ):
            raise EClassError("Session file is invalid")
        return cls(
            cookies=[_compact_cookie(cookie) for cookie in cookies if _is_cau_domain(cookie.get("domain"))],
            saved_at=value.get("saved_at", ""),
            browser=value.get("browser"),
        )

    def to_json(self) -> dict:
        return {
            "version": 1,
            "saved_at": self.saved_at,
            "browser": self.browser,
            "cookies": self.cookies,
        }

    def token(self, name: str) -> str | None:
        now = time.time()
        for cookie in self.cookies:
            if cookie.get("name") != name:
                continue
            expires = cookie.get("expires")
            if isinstance(expires, (int, float)) and 0 < expires <= now:
                continue
            return str(cookie.get("value") or "")
        return None


def load_session(path: Path | None = None) -> Session | None:
    path = path or default_session_file()
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, ValueError):
        raise EClassError(
            f"Session file could not be read ({path}); run `eclass-mcp login` again"
        ) from None
    return Session.from_json(value)


def save_session(session: Session, path: Path | None = None) -> Path:
    """Write a session atomically, owner-only. Used by tests and optional tooling."""
    path = path or default_session_file()
    session.saved_at = now_iso()
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = path.with_name(f".{path.name}.tmp")
    try:
        temporary.write_text(
            json.dumps(session.to_json(), ensure_ascii=False, separators=(",", ":")),
            encoding="utf-8",
        )
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
        os.chmod(path, 0o600)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return path
