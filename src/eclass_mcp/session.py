"""Reusable e-Class browser session without storing an account password.

The browser remains the only place a user signs in.  A local, owner-only JSON
file keeps the resulting cookies so later MCP runs can use plain HTTP instead of
driving a browser tab for every request.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit

import httpx

from .cdp import validate_endpoint
from .transport import ORIGIN, EClassError

COOKIE_DOMAIN = "cau.ac.kr"
USER_AGENT = (
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
    "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36"
)
JSON_ACCEPT = "application/json+canvas-string-ids"


class SessionExpired(EClassError):
    """The saved browser session cannot be used for an authenticated request."""


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
    endpoint: str | None = None

    @classmethod
    def from_cdp(cls, cookies, endpoint: str | None = None) -> "Session":
        values = {}
        for raw in cookies or []:
            if not isinstance(raw, dict) or not raw.get("name") or not _is_cau_domain(raw.get("domain")):
                continue
            cookie = _compact_cookie(raw)
            key = (cookie["name"], _cookie_domain(cookie.get("domain")), cookie.get("path") or "/")
            values[key] = cookie
        return cls(
            sorted(values.values(), key=lambda item: (item["name"], item.get("domain", ""))),
            endpoint=endpoint,
        )

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
            cookies=[_compact_cookie(cookie) for cookie in cookies],
            saved_at=value.get("saved_at", ""),
            endpoint=value.get("endpoint"),
        )

    def to_json(self) -> dict:
        return {
            "version": 1,
            "saved_at": self.saved_at,
            "endpoint": self.endpoint,
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

    def upsert(self, cookie: dict) -> None:
        key = (cookie.get("name"), _cookie_domain(cookie.get("domain")), cookie.get("path") or "/")
        self.cookies = [
            item for item in self.cookies
            if (item.get("name"), _cookie_domain(item.get("domain")), item.get("path") or "/") != key
        ]
        self.cookies.append(_compact_cookie(cookie))
        self.cookies.sort(key=lambda item: (item.get("name", ""), item.get("domain", "")))

    def as_cookies(self) -> httpx.Cookies:
        jar = httpx.Cookies()
        now = time.time()
        for cookie in self.cookies:
            expires = cookie.get("expires")
            if isinstance(expires, (int, float)) and 0 < expires <= now:
                continue
            domain = cookie.get("domain") or "eclass3.cau.ac.kr"
            jar.set(
                str(cookie["name"]),
                str(cookie.get("value") or ""),
                domain=str(domain),
                path=str(cookie.get("path") or "/"),
            )
        return jar


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
    path = path or default_session_file()
    session.saved_at = now_iso()
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    try:
        os.chmod(path.parent, 0o700)
    except OSError:
        pass
    descriptor, temporary = tempfile.mkstemp(prefix=".session-", suffix=".tmp", dir=path.parent)
    try:
        try:
            os.fchmod(descriptor, 0o600)
        except (AttributeError, OSError, NotImplementedError):
            pass
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            json.dump(session.to_json(), stream, ensure_ascii=False, separators=(",", ":"))
        os.replace(temporary, path)
        os.chmod(path, 0o600)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise
    return path


def delete_session(path: Path | None = None) -> bool:
    path = path or default_session_file()
    try:
        path.unlink()
        return True
    except FileNotFoundError:
        return False


def merge_cookies(session: Session, jar: httpx.Cookies) -> None:
    """Copy cookies set or refreshed by an HTTP response into the session."""
    for cookie in jar.jar:
        if not _is_cau_domain(cookie.domain):
            continue
        session.upsert(
            {
                "name": cookie.name,
                "value": cookie.value,
                "domain": cookie.domain,
                "path": cookie.path or "/",
                "expires": cookie.expires,
                "secure": cookie.secure,
                "httpOnly": cookie.has_nonstandard_attr("HttpOnly"),
            }
        )


def _headers() -> dict:
    return {
        "Accept": JSON_ACCEPT,
        "Accept-Language": "ko,en-US;q=0.9,en;q=0.8",
        "Referer": ORIGIN + "/",
        "User-Agent": USER_AGENT,
    }


def _parse_profile(response: httpx.Response):
    if response.status_code != 200 or "html" in response.headers.get("content-type", "").lower():
        return None
    value = response.text.lstrip("\ufeff").lstrip()
    if value.startswith("while(1);"):
        value = value[9:].lstrip()
    try:
        profile = json.loads(value)
    except ValueError:
        return None
    return profile if isinstance(profile, dict) and profile.get("id") else None


async def verify_session(session: Session) -> bool:
    async with httpx.AsyncClient(
        cookies=session.as_cookies(), timeout=15, follow_redirects=False, trust_env=False
    ) as client:
        try:
            response = await client.get(ORIGIN + "/api/v1/users/self/profile", headers=_headers())
        except httpx.HTTPError:
            return False
        merge_cookies(session, client.cookies)
        return _parse_profile(response) is not None


async def ensure_learningx_token(session: Session) -> Session:
    """Ask LearningX for its API token when the login flow has not set it yet."""
    if session.token("xn_api_token"):
        return session
    async with httpx.AsyncClient(
        cookies=session.as_cookies(), timeout=15, follow_redirects=True, trust_env=False
    ) as client:
        for path in ("/learningx/main", "/learningx/dashboard"):
            try:
                await client.get(ORIGIN + path, headers=_headers())
            except httpx.HTTPError:
                continue
            merge_cookies(session, client.cookies)
            if session.token("xn_api_token"):
                break
    return session


async def harvest_cdp(endpoint: str | None = None) -> list[dict]:
    """Read cookies from a local Chromium DevTools endpoint (including HttpOnly)."""
    endpoint = validate_endpoint(
        endpoint or os.getenv("ECLASS_CDP_ENDPOINT", "http://127.0.0.1:9222")
    ).rstrip("/")
    parsed = urlsplit(endpoint)
    if parsed.scheme in ("ws", "wss"):
        endpoint = ("https" if parsed.scheme == "wss" else "http") + "://" + parsed.netloc
    try:
        async with httpx.AsyncClient(timeout=8, trust_env=False) as client:
            response = await client.get(endpoint + "/json/version")
            response.raise_for_status()
            websocket_url = response.json()["webSocketDebuggerUrl"]
    except (httpx.HTTPError, KeyError, ValueError):
        raise EClassError(
            "Cannot reach the local Chromium debugging port; start the login browser first"
        ) from None

    from websockets.asyncio.client import connect

    try:
        async with connect(websocket_url, max_size=8 * 1024 * 1024, open_timeout=8) as socket:
            await socket.send(json.dumps({"id": 1, "method": "Storage.getCookies", "params": {}}))
            while True:
                message = json.loads(await asyncio.wait_for(socket.recv(), timeout=10))
                if message.get("id") != 1:
                    continue
                if message.get("error"):
                    raise EClassError("Chromium did not return its cookie store")
                cookies = message.get("result", {}).get("cookies", [])
                if not isinstance(cookies, list):
                    raise EClassError("Chromium returned an invalid cookie store")
                return cookies
    except EClassError:
        raise
    except Exception:
        # WebSocket errors can include the debugging URL; keep the message generic.
        raise EClassError("Could not read cookies from the local Chromium debugging port") from None


async def login_via_cdp(
    endpoint: str | None = None,
    timeout: float = 300,
    interval: float = 2,
    wait_for_browser: bool = True,
) -> Session:
    """Wait for a human browser login, then snapshot its cookies."""
    endpoint = validate_endpoint(
        endpoint or os.getenv("ECLASS_CDP_ENDPOINT", "http://127.0.0.1:9222")
    ).rstrip("/")
    loop = asyncio.get_running_loop()
    deadline = loop.time() + max(1, timeout)
    last_error: EClassError | None = None
    while loop.time() < deadline:
        try:
            cookies = await harvest_cdp(endpoint)
            session = Session.from_cdp(cookies, endpoint=endpoint)
            if await verify_session(session):
                await ensure_learningx_token(session)
                return session
            last_error = SessionExpired("The browser is not logged in to e-Class yet")
        except SessionExpired as error:
            last_error = error
        except EClassError as error:
            if not wait_for_browser and "Cannot reach" in str(error):
                raise
            last_error = error
        await asyncio.sleep(max(0.2, interval))
    reason = str(last_error) if last_error else "login was not completed"
    raise EClassError(f"Login was not detected ({reason}); run `eclass-mcp login` again") from None


async def refresh_session(endpoint: str | None = None, path: Path | None = None) -> Session:
    """Reuse an already open login browser; fail immediately when it is absent."""
    endpoint = validate_endpoint(
        endpoint or os.getenv("ECLASS_CDP_ENDPOINT", "http://127.0.0.1:9222")
    ).rstrip("/")
    cookies = await harvest_cdp(endpoint)
    session = Session.from_cdp(cookies, endpoint=endpoint)
    if not await verify_session(session):
        raise SessionExpired("The open browser is not logged in to e-Class")
    await ensure_learningx_token(session)
    save_session(session, path)
    return session


def session_endpoint() -> str:
    return validate_endpoint(os.getenv("ECLASS_CDP_ENDPOINT", "http://127.0.0.1:9222"))
