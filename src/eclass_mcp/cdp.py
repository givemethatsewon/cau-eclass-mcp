"""Attach to a user's local Chromium; never launch or close their browser."""

from __future__ import annotations

import asyncio
import ipaddress
import os
import re
from contextlib import asynccontextmanager
from pathlib import Path
from urllib.parse import urljoin, urlsplit

import httpx

from .browser_scripts import READ_SCRIPT, SUBMIT_SCRIPT, VIDEO_SCRIPT
from .transport import ORIGIN, EClassError, local_path

DOWNLOAD_HOSTS = {"eclass3.cau.ac.kr", "ocs.cau.ac.kr", "cau-cms-object.cdn.gov-ntruss.com"}


def validate_endpoint(endpoint: str) -> str:
    try:
        url = urlsplit(endpoint)
        loopback = url.hostname == "localhost" or ipaddress.ip_address(url.hostname or "").is_loopback
        port = url.port
    except ValueError:
        raise EClassError("CDP endpoint must use localhost or a loopback IP") from None
    if not loopback or url.scheme not in {"http", "https", "ws", "wss"} or not port:
        raise EClassError("CDP endpoint must be a local HTTP/WebSocket URL with a port")
    if url.username or url.password or url.query or url.fragment:
        raise EClassError("CDP endpoint must not contain credentials, query parameters or fragments")
    return endpoint


def trusted_download(url: str) -> str:
    parsed = urlsplit(url)
    if (
        parsed.scheme != "https"
        or parsed.hostname not in DOWNLOAD_HOSTS
        or parsed.username
        or parsed.password
    ):
        raise EClassError("Untrusted download host")
    if parsed.port not in (None, 443):
        raise EClassError("Unexpected download port")
    return url


class CDPTransport:
    def __init__(self, endpoint: str | None = None):
        self.endpoint = validate_endpoint(
            endpoint or os.getenv("ECLASS_CDP_ENDPOINT", "http://127.0.0.1:9222")
        )
        self.lock = asyncio.Lock()
        self.playwright = None
        self.browser = None

    async def __aenter__(self):
        try:
            from playwright.async_api import async_playwright
        except ImportError:
            raise EClassError("CDP needs the optional dependency: install cau-eclass-mcp[cdp]") from None
        try:
            self.playwright = await async_playwright().start()
            self.browser = await self.playwright.chromium.connect_over_cdp(self.endpoint, timeout=15000)
            return self
        except Exception:
            await self.__aexit__(None, None, None)
            raise EClassError(
                "Cannot connect to local Chromium; check the CDP port and dedicated profile"
            ) from None

    async def __aexit__(self, *args):
        # Stopping the client disconnects CDP; Browser.close() would close a user's browser.
        if self.playwright:
            await self.playwright.stop()
        self.playwright = self.browser = None

    def page(self):
        if self.browser is None or not self.browser.is_connected():
            raise EClassError("CDP is disconnected; reconnect the MCP server")
        matches = [
            (ctx, p) for ctx in self.browser.contexts for p in ctx.pages if p.url.startswith(ORIGIN + "/")
        ]
        if not matches:
            raise EClassError("Open a logged-in e-Class tab in the CDP browser first")
        if len({id(ctx) for ctx, _ in matches}) > 1:
            raise EClassError(
                "Multiple e-Class browser contexts found; use a dedicated profile with one account"
            )
        return matches[0][1]

    @asynccontextmanager
    async def operation(self):
        async with self.lock:
            try:
                async with asyncio.timeout(180):
                    yield self.page()
            except EClassError:
                raise
            except Exception:
                # Playwright/HTTP exceptions can include signed URLs or form values.
                raise EClassError(
                    "CDP operation failed; check login, page availability and browser connection"
                ) from None

    async def get(self, path: str, learningx: bool = False, html: bool = False):
        payload = {"path": local_path(path), "learningx": learningx, "html": html}
        async with self.operation() as page:
            target = (
                next((f for f in page.frames if f.url.startswith(ORIGIN + "/learningx/")), page)
                if learningx
                else page
            )
            value = await target.evaluate(READ_SCRIPT, payload)
            if value["status"] >= 400:
                raise EClassError(
                    f"e-Class HTTP {value['status']} (session, permission, or unsupported endpoint)"
                )
            if value["body"] is None:
                raise EClassError("Expected JSON was not returned; login may have expired")
            return value

    @asynccontextmanager
    async def response(self, context, url):
        # Filter cookies for each redirect target. No Canvas bearer/CSRF header is forwarded.
        async with httpx.AsyncClient(timeout=60, follow_redirects=False, trust_env=False) as client:
            for _ in range(6):
                target = trusted_download(url)
                cookies = await context.cookies([target])
                headers = (
                    {"Cookie": "; ".join(f"{c['name']}={c['value']}" for c in cookies)} if cookies else {}
                )
                async with client.stream("GET", target, headers=headers) as response:
                    if response.is_redirect:
                        location = response.headers.get("location")
                        if not location:
                            raise EClassError("Download redirect has no destination")
                        url = urljoin(target, location)
                        continue
                    yield response
                    return
            raise EClassError("Too many download redirects")

    @staticmethod
    async def save_response(response, destination, limit, expected=None):
        content_type = response.headers.get("content-type", "").lower()
        if not response.is_success or "html" in content_type or "json" in content_type:
            raise EClassError("Download did not return a file")
        if int(response.headers.get("content-length", 0)) > limit:
            raise EClassError("File exceeds download limit")
        created = False
        try:
            with destination.open("xb") as stream:
                created = True
                os.chmod(destination, 0o600)
                size = 0
                async for chunk in response.aiter_bytes():
                    size += len(chunk)
                    if size > limit:
                        raise EClassError("File exceeds download limit")
                    stream.write(chunk)
            if not size or (expected and size != expected):
                raise EClassError("Downloaded file size mismatch")
            return {"bytes": size, "content_type": content_type}
        except BaseException:
            if created:
                destination.unlink(missing_ok=True)
            raise

    async def download(self, path: str, destination: Path, max_bytes: int):
        url = ORIGIN + local_path(path)
        async with self.operation() as page:
            async with self.response(page.context, url) as response:
                return await self.save_response(response, destination, max_bytes)

    async def small_text(self, context, url):
        async with self.response(context, url) as response:
            if not response.is_success:
                raise EClassError("Playback metadata request failed")
            chunks, size = [], 0
            async for chunk in response.aiter_bytes():
                size += len(chunk)
                if size > 2 * 1024 * 1024:
                    raise EClassError("Playback metadata exceeds limit")
                chunks.append(chunk)
            return b"".join(chunks).decode("utf-8")

    async def playback_url(self, context, view_url):
        html = await self.small_text(context, view_url)
        match = re.search(r"var contentUri\s*=\s*'([^']+)'", html)
        if not match:
            raise EClassError("Playback metadata is unsupported")
        xml = await self.small_text(context, urljoin(view_url, match[1]))
        template = re.search(r'<media_uri method="progressive" target="all">([^<]+)', xml)
        files = re.findall(r'<main_media media_id="[^"]+">([^<]+)', xml)
        if not template or len(files) != 1 or not re.fullmatch(r"[-a-zA-Z0-9_.]+\.mp4", files[0]):
            raise EClassError("Only a single provided playback MP4 is supported")
        return trusted_download(template[1].replace("[MEDIA_FILE]", files[0]))

    async def video_original(self, course_id, item_id, destination=None, allow_playback_copy=False):
        async with self.operation() as page:
            media = await page.evaluate(VIDEO_SCRIPT, {"course": course_id, "item": item_id})
            url, view_url = media.pop("url", None), media.pop("view_url", None)
            if destination is None or not media.get("available"):
                return media
            limit = 1024 * 1024 * 1024
            if (media.get("expected_bytes") or 0) > limit:
                raise EClassError("File exceeds download limit")
            async with self.response(page.context, url) as response:
                if response.is_success:
                    result = await self.save_response(
                        response, destination, limit, media.get("expected_bytes")
                    )
                    return {**media, **result, "source": "commons_original"}
            if not allow_playback_copy:
                raise EClassError("Original download endpoint failed; optionally allow a playback MP4 copy")
            url = await self.playback_url(page.context, view_url)
            async with self.response(page.context, url) as response:
                result = await self.save_response(response, destination, limit)
            return {**media, **result, "source": "commons_playback_mp4"}

    async def syllabus_pdf(self, course_id: int, destination: Path):
        async with self.operation() as page:
            tab = await page.context.new_page()
            try:
                await tab.goto(
                    f"{ORIGIN}/courses/{course_id}/assignments/syllabus", wait_until="domcontentloaded"
                )
                frame = tab.frame_locator('iframe[src^="https://rpt80.cau.ac.kr/oz80/ozViewer.jsp"]')
                await frame.locator('input.btnSAVEAS[title="Save"]').click(timeout=30000)
                dialog = frame.get_by_role("dialog", name="Save", exact=True)
                await dialog.locator("select.oz_ui_layout").select_option(label="Adobe PDF File(*.pdf)")
                async with tab.expect_download(timeout=60000) as pending:
                    await dialog.get_by_role("button", name="OK", exact=True).click()
                download = await pending.value
                if await download.failure():
                    raise EClassError("PDF download failed")
                # Copy exclusively instead of save_as overwriting an existing path.
                source = await download.path()
                if source is None or Path(source).stat().st_size > 64 * 1024 * 1024:
                    raise EClassError("PDF download missing or oversized")
                import shutil

                created = False
                try:
                    with destination.open("xb") as target, Path(source).open("rb") as stream:
                        created = True
                        os.chmod(destination, 0o600)
                        shutil.copyfileobj(stream, target)
                except BaseException:
                    if created:
                        destination.unlink(missing_ok=True)
                    raise
                finally:
                    await download.delete()
                return {"bytes": destination.stat().st_size, "content_type": "application/pdf"}
            finally:
                await tab.close()

    async def submit_files(self, payload):
        """Transmit the service's verified byte snapshot without reopening local files."""
        async with self.operation() as page:
            return await page.evaluate(SUBMIT_SCRIPT, payload)
