"""HTTP-first e-Class transport using a saved, logged-in browser session.

Only a browser can establish the school login.  After that, ordinary reads and
downloads do not need a browser process, page, or per-request JavaScript.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
from html import unescape
from pathlib import Path
from urllib.parse import quote, urljoin, urlsplit

import httpx

from .cdp import trusted_download
from .session import (
    JSON_ACCEPT,
    USER_AGENT,
    Session,
    SessionExpired,
    default_session_file,
    load_session,
    merge_cookies,
    refresh_session,
    session_endpoint,
)
from .transport import ORIGIN, EClassError, local_path

MAX_JSON_BYTES = 8 * 1024 * 1024
MAX_HTML_BYTES = 5 * 1024 * 1024
MAX_METADATA_BYTES = 2 * 1024 * 1024


def _item_id(value) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int) and value > 0:
        return value
    if isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*", value):
        return int(value)
    return None


def _locked(value: dict) -> bool:
    return (
        value.get("opened") is False
        or value.get("published") is False
        or value.get("locked") is True
        or value.get("locked_for_user") is True
        or value.get("lecture_period_status") in {"locked", "not_open", "before_open", "closed"}
    )


def _find_item(value, item_id: int) -> dict | None:
    if not isinstance(value, dict):
        if isinstance(value, list):
            for entry in value:
                found = _find_item(entry, item_id)
                if found:
                    return found
        return None
    if _item_id(value.get("item_id")) == item_id and isinstance(value.get("item_content_data"), dict):
        return value
    for entry in value.values():
        found = _find_item(entry, item_id)
        if found:
            return found
    return None


def _text_body(response: httpx.Response) -> str:
    value = response.text.lstrip("\ufeff").lstrip()
    if value.startswith("while(1);"):
        value = value[9:].lstrip()
    return value


def _json_body(response: httpx.Response):
    try:
        return json.loads(_text_body(response))
    except ValueError:
        raise EClassError("Expected JSON was not returned; login may have expired") from None


def _declared_length(response: httpx.Response) -> int:
    try:
        return int(response.headers.get("content-length", 0))
    except (TypeError, ValueError):
        return 0


def _endpoint_unavailable(error: EClassError) -> bool:
    """Fallback is only for an endpoint failure, never for invalid bytes we received."""
    text = str(error)
    return any(token in text for token in ("did not return a file", "request failed", "redirect"))


class HTTPTransport:
    kind = "http"

    def __init__(
        self,
        session: Session | None = None,
        endpoint: str | None = None,
        session_file: Path | None = None,
        client: httpx.AsyncClient | None = None,
    ):
        self.session = session
        self.session_file = session_file or default_session_file()
        self.endpoint = endpoint or session_endpoint()
        self._client = client
        self._owns_client = client is None
        self._browser = None
        self._refresh_lock = asyncio.Lock()
        self._auto_refresh = os.getenv("ECLASS_AUTO_REFRESH", "1") == "1"
        self.on_refresh = None

    @classmethod
    def from_env(cls) -> "HTTPTransport":
        path = default_session_file()
        session = load_session(path)
        if session is None:
            raise EClassError(
                "No saved e-Class session. Run `eclass-mcp login` once, or set "
                "ECLASS_BROWSER=aside to use the legacy Aside adapter."
            )
        return cls(session=session, endpoint=session.endpoint, session_file=path)

    async def __aenter__(self):
        if self._client is None and self.session is None:
            raise EClassError("No saved e-Class session; run `eclass-mcp login` once")
        if self._client is None:
            self._client = httpx.AsyncClient(
                cookies=self.session.as_cookies(),
                timeout=httpx.Timeout(30, connect=10),
                limits=httpx.Limits(max_connections=8, max_keepalive_connections=4),
                follow_redirects=False,
                trust_env=False,
            )
            self._owns_client = True
        return self

    async def __aexit__(self, *args):
        if self._browser is not None:
            await self._browser.__aexit__(None, None, None)
            self._browser = None
        if self._client is not None and self._owns_client:
            await self._client.aclose()
        self._client = None
        return None

    def _headers(self, path: str, html: bool, learningx: bool) -> dict:
        headers = {
            "Accept": "text/html,application/xhtml+xml" if html else JSON_ACCEPT,
            "Accept-Language": "ko,en-US;q=0.9,en;q=0.8",
            "Referer": ORIGIN + "/",
            "User-Agent": USER_AGENT,
        }
        if learningx or path.startswith("/learningx/"):
            token = self.session.token("xn_api_token") if self.session else None
            if token:
                headers["Authorization"] = "Bearer " + token
        return headers

    async def refresh(self) -> Session:
        async with self._refresh_lock:
            session = await refresh_session(self.endpoint, self.session_file)
            self.session = session
            if self._client is not None:
                self._client.cookies = session.as_cookies()
            if self.on_refresh is not None:
                self.on_refresh()
            return session

    async def _try_refresh(self) -> bool:
        if not self._auto_refresh or self.session is None:
            return False
        try:
            await self.refresh()
            return True
        except EClassError:
            return False

    async def _request(self, path: str, html: bool, learningx: bool):
        if self._client is None:
            raise EClassError("HTTP transport is not connected")
        current = ORIGIN + local_path(path)
        for _ in range(5):
            try:
                response = await self._client.get(
                    current, headers=self._headers(path, html, learningx), follow_redirects=False
                )
            except httpx.HTTPError:
                raise EClassError("e-Class network request failed") from None
            if response.is_redirect:
                location = response.headers.get("location")
                if not location:
                    raise EClassError("e-Class redirect has no destination")
                target = urljoin(current, location)
                parsed = urlsplit(target)
                if (
                    parsed.scheme != "https"
                    or parsed.netloc != urlsplit(ORIGIN).netloc
                    or parsed.username
                    or parsed.password
                ):
                    raise EClassError("Cross-origin e-Class redirect rejected")
                if "login" in parsed.path.casefold():
                    raise SessionExpired("e-Class redirected to login; run `eclass-mcp login`")
                current = target
                path = parsed.path + (("?" + parsed.query) if parsed.query else "")
                continue
            if "set-cookie" in response.headers and self.session is not None:
                merge_cookies(self.session, self._client.cookies)
            return response
        raise EClassError("Too many e-Class redirects")

    async def get(self, path: str, learningx: bool = False, html: bool = False):
        for attempt in range(2):
            try:
                response = await self._request(path, html, learningx)
            except SessionExpired:
                if attempt == 0 and await self._try_refresh():
                    continue
                raise
            if response.status_code == 401 and attempt == 0 and await self._try_refresh():
                continue
            if response.status_code in (401, 403):
                if response.status_code == 401:
                    raise SessionExpired(
                        "e-Class session expired; run `eclass-mcp login` while the browser is open"
                    )
                raise EClassError("e-Class HTTP 403 (permission or unsupported endpoint)")
            if response.status_code >= 400:
                raise EClassError(
                    f"e-Class HTTP {response.status_code} (session, permission, or unsupported endpoint)"
                )
            content_type = response.headers.get("content-type", "").lower()
            if not html and "html" in content_type:
                if attempt == 0 and await self._try_refresh():
                    continue
                raise SessionExpired(
                    "e-Class returned a login page; run `eclass-mcp login` and try again"
                )
            if len(response.content) > (MAX_HTML_BYTES if html else MAX_JSON_BYTES):
                raise EClassError("e-Class response exceeds the read limit")
            return {
                "status": response.status_code,
                "body": response.text if html else _json_body(response),
                "content_type": content_type,
                "link": response.headers.get("link"),
            }
        raise SessionExpired("e-Class session could not be refreshed; run `eclass-mcp login`")

    async def _fetch_text(self, url: str, limit: int = MAX_METADATA_BYTES) -> str:
        if self._client is None:
            raise EClassError("HTTP transport is not connected")
        target = trusted_download(url)
        for _ in range(6):
            try:
                async with self._client.stream(
                    "GET",
                    target,
                    headers={"Accept": "*/*", "Referer": ORIGIN + "/", "User-Agent": USER_AGENT},
                    follow_redirects=False,
                ) as response:
                    if response.is_redirect:
                        location = response.headers.get("location")
                        if not location:
                            raise EClassError("Download redirect has no destination")
                        target = trusted_download(urljoin(target, location))
                        continue
                    if not response.is_success:
                        raise EClassError("Download metadata request failed")
                    chunks, size = [], 0
                    async for chunk in response.aiter_bytes():
                        size += len(chunk)
                        if size > limit:
                            raise EClassError("Download metadata exceeds the read limit")
                        chunks.append(chunk)
                    return b"".join(chunks).decode("utf-8", "replace")
            except httpx.HTTPError:
                raise EClassError("Download metadata request failed") from None
        raise EClassError("Too many download redirects")

    async def _download_url(
        self, url: str, destination: Path, max_bytes: int, expected: int | None = None
    ) -> dict:
        if max_bytes <= 0:
            raise EClassError("Download limit must be positive")
        if self._client is None:
            raise EClassError("HTTP transport is not connected")
        target = trusted_download(url)
        for _ in range(6):
            try:
                async with self._client.stream(
                    "GET",
                    target,
                    headers={"Accept": "*/*", "Referer": ORIGIN + "/", "User-Agent": USER_AGENT},
                    follow_redirects=False,
                ) as response:
                    if response.is_redirect:
                        location = response.headers.get("location")
                        if not location:
                            raise EClassError("Download redirect has no destination")
                        target = trusted_download(urljoin(target, location))
                        continue
                    content_type = response.headers.get("content-type", "").lower()
                    if not response.is_success or "html" in content_type or "json" in content_type:
                        raise EClassError("Download did not return a file")
                    if _declared_length(response) > max_bytes:
                        raise EClassError("File exceeds download limit")
                    created = False
                    try:
                        with destination.open("xb") as stream:
                            created = True
                            os.chmod(destination, 0o600)
                            size = 0
                            async for chunk in response.aiter_bytes():
                                size += len(chunk)
                                if size > max_bytes:
                                    raise EClassError("File exceeds download limit")
                                stream.write(chunk)
                        if not size or (expected is not None and size != expected):
                            raise EClassError("Downloaded file size mismatch")
                        return {"bytes": size, "content_type": content_type}
                    except BaseException:
                        if created:
                            destination.unlink(missing_ok=True)
                        raise
            except httpx.HTTPError:
                raise EClassError("Download request failed") from None
        raise EClassError("Too many download redirects")

    async def download(self, path: str, destination: Path, max_bytes: int):
        return await self._download_url(ORIGIN + local_path(path), destination, max_bytes)

    async def _modules(self, course_id: int):
        value = await self.get(
            f"/learningx/api/v1/courses/{course_id}/modules?include_detail=true", learningx=True
        )
        if not isinstance(value["body"], list):
            raise EClassError("Unexpected LearningX module response")
        return value["body"]

    async def _media_info(self, course_id: int, item_id: int, kind: str):
        modules = await self._modules(course_id)
        item = _find_item(modules, item_id)
        if not item:
            return None, {"available": False, "reason": f"{kind} item not found in course"}
        if item.get("course_id") is not None and _item_id(item.get("course_id")) != course_id:
            return None, {"available": False, "reason": f"{kind} item not found in this course"}
        data = item.get("item_content_data") or {}
        if _locked(item) or _locked(data) or not data.get("content_id") or data["content_id"] == "not_open":
            return None, {"available": False, "reason": f"{kind} is not open to this user"}
        return item, data

    async def _commons_metadata(self, content_id) -> dict:
        path = "/learningx/api/v1/commons/contents?content_id=" + quote(str(content_id), safe="")
        value = await self.get(path, learningx=True)
        body = value["body"]
        result = body.get("result") if isinstance(body, dict) else None
        if not isinstance(result, dict):
            raise EClassError("Commons metadata was not returned")
        return result

    @staticmethod
    def _expected(value) -> int | None:
        try:
            number = int(value)
        except (TypeError, ValueError):
            return None
        return number if number > 0 else None

    async def commons_pdf(
        self, course_id: int, item_id: int, destination: Path, max_bytes: int = 64 * 1024 * 1024
    ) -> dict:
        if max_bytes <= 0:
            raise EClassError("Download limit must be positive")
        item, data = await self._media_info(course_id, item_id, "PDF")
        if item is None:
            return data
        if item.get("item_content_type") != "commons" or data.get("content_type") != "pdf":
            return {"available": False, "reason": "Only Commons PDF items are supported"}
        metadata = await self._commons_metadata(data["content_id"])
        if (
            metadata.get("is_original_file_deleted")
            or str(metadata.get("extension", "")).casefold() != "pdf"
        ):
            return {
                "available": False,
                "reason": "A native original PDF is unavailable; rendered pages are not synthesized",
            }
        expected = self._expected(metadata.get("original_file_length"))
        if expected is not None and expected > max_bytes:
            raise EClassError("File exceeds download limit")
        info = {
            "available": True,
            "title": item.get("title"),
            "source": "commons_original",
            "original": True,
            "expected_bytes": expected,
        }
        download_url = metadata.get("download_url")
        view_url = metadata.get("view_url")
        if download_url:
            try:
                result = await self._download_url(download_url, destination, max_bytes, expected)
                return {**info, **result}
            except FileExistsError:
                raise
            except EClassError as error:
                if not view_url or not _endpoint_unavailable(error):
                    raise
        if not view_url:
            raise EClassError(
                "Native PDF download endpoint failed and no viewer metadata was provided"
            )
        html = await self._fetch_text(view_url)
        match = re.search(r"var\s+contentUri\s*=\s*['\"]([^'\"]+)['\"]", html)
        if not match:
            raise EClassError("Native PDF viewer metadata is unsupported")
        xml = await self._fetch_text(urljoin(view_url, unescape(match[1])))
        match = re.search(r"<content_download_uri>\s*(.*?)\s*</content_download_uri>", xml, re.S)
        if not match:
            raise EClassError("Viewer metadata does not provide a native PDF download")
        value = match[1]
        if value.startswith("<![CDATA[") and value.endswith("]]>"):
            value = value[9:-3]
        target = urljoin(view_url, unescape(value))
        result = await self._download_url(target, destination, max_bytes, expected)
        return {**info, **result}

    async def _playback_url(self, view_url: str) -> str:
        html = await self._fetch_text(view_url)
        match = re.search(r"var contentUri\s*=\s*'([^']+)'", html)
        if not match:
            raise EClassError("Playback metadata is unsupported")
        xml = await self._fetch_text(urljoin(view_url, unescape(match[1])))
        template = re.search(r'<media_uri method="progressive" target="all">([^<]+)', xml)
        files = re.findall(r'<main_media media_id="[^"]+">([^<]+)', xml)
        if not template or len(files) != 1 or not re.fullmatch(r"[-a-zA-Z0-9_.]+\.mp4", files[0]):
            raise EClassError("Only a single provided playback MP4 is supported")
        return trusted_download(template[1].replace("[MEDIA_FILE]", files[0]))

    async def video_original(
        self, course_id: int, item_id: int, destination: Path | None = None,
        allow_playback_copy: bool = False,
    ):
        item, data = await self._media_info(course_id, item_id, "Video")
        if item is None:
            return data
        if data.get("content_type") != "mp4":
            return {"available": False, "reason": "Only original MP4 downloads are supported"}
        metadata = await self._commons_metadata(data["content_id"])
        if (
            not metadata
            or metadata.get("is_original_file_deleted")
            or not metadata.get("download_url")
        ):
            return {"available": False, "reason": "Original file is unavailable"}
        expected = self._expected(metadata.get("original_file_length"))
        info = {
            "available": True,
            "title": item.get("title"),
            "expected_bytes": expected,
        }
        if destination is None:
            return info
        limit = 1024 * 1024 * 1024
        if expected is not None and expected > limit:
            raise EClassError("File exceeds download limit")
        url = trusted_download(metadata["download_url"])
        if urlsplit(url).hostname != "ocs.cau.ac.kr":
            raise EClassError("Untrusted download host")
        try:
            result = await self._download_url(url, destination, limit, expected)
            return {**info, **result, "source": "commons_original", "original": True}
        except EClassError as error:
            if not _endpoint_unavailable(error):
                raise
            if not allow_playback_copy or not metadata.get("view_url"):
                raise EClassError(
                    "Original download endpoint failed; optionally allow a playback MP4 copy"
                ) from error
        url = await self._playback_url(metadata["view_url"])
        result = await self._download_url(url, destination, limit)
        return {
            **info,
            **result,
            "source": "commons_playback_mp4",
            "expected_bytes": None,
            "original": False,
        }

    async def _ensure_browser(self):
        if self._browser is not None:
            return self._browser
        backend = os.getenv("ECLASS_FALLBACK_BROWSER", "cdp").casefold()
        if backend == "aside":
            from .transport import AsideTransport

            browser = AsideTransport()
        elif backend == "cdp":
            from .cdp import CDPTransport

            browser = CDPTransport(self.endpoint)
        else:
            raise EClassError("ECLASS_FALLBACK_BROWSER must be aside or cdp")
        await browser.__aenter__()
        self._browser = browser
        return browser

    async def syllabus_pdf(self, course_id: int, destination: Path):
        """OZ's native PDF export is a browser UI flow; use the connected browser."""
        browser = await self._ensure_browser()
        return await browser.syllabus_pdf(course_id, destination)

    async def submit_files(self, payload):
        """Assignment upload keeps the existing browser-verified write path."""
        browser = await self._ensure_browser()
        return await browser.submit_files(payload)
