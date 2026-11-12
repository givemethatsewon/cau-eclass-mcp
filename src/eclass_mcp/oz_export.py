"""Small Playwright helper: export one e-Class syllabus through OZ's PDF Save.

The Rust MCP server owns authentication, HTTP reads, downloads, and submissions.
This helper is invoked only for the OZ viewer, whose PDF is generated in the
browser by OZJSViewer. It uses the existing JSON session and never asks for a
password, OTP, cookie, or session value.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import sys
import time
from pathlib import Path
from uuid import uuid4

from .errors import ORIGIN, EClassError
from .session import Session, default_session_file, load_session

MAX_PDF_BYTES = 64 * 1024 * 1024
OZ_PREFIX = "https://rpt80.cau.ac.kr/oz80/ozViewer.jsp"
DEFAULT_TIMEOUT = 90


def find_browser(explicit: str | None = None) -> Path | None:
    """Return an installed Chromium executable, or None for Playwright's bundled one."""
    configured = explicit or os.getenv("ECLASS_OZ_BROWSER") or os.getenv("ECLASS_BROWSER_PATH")
    if configured:
        path = Path(configured).expanduser()
        if path.is_file():
            return path
        raise EClassError("The configured OZ browser executable does not exist")
    candidates: list[Path] = []
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
    else:
        import shutil

        for name in ("google-chrome", "google-chrome-stable", "chromium", "chromium-browser"):
            if found := shutil.which(name):
                candidates.append(Path(found))
    return next((path for path in candidates if path.is_file()), None)


def playwright_cookies(session: Session) -> list[dict]:
    """Convert the owner-only session file into Playwright cookie objects."""
    now = time.time()
    cookies: list[dict] = []
    for raw in session.cookies:
        name, value = raw.get("name"), raw.get("value")
        domain = str(raw.get("domain") or "").lstrip(".").lower()
        if not name or value is None or not (domain == "cau.ac.kr" or domain.endswith(".cau.ac.kr")):
            continue
        expires = raw.get("expires")
        if isinstance(expires, (int, float)) and 0 < expires <= now:
            continue
        cookie = {
            "name": str(name),
            "value": str(value),
            "domain": str(raw.get("domain") or domain),
            "path": str(raw.get("path") or "/"),
            "secure": bool(raw.get("secure")),
            "httpOnly": bool(raw.get("httpOnly")),
        }
        if isinstance(expires, (int, float)) and expires > now:
            cookie["expires"] = float(expires)
        same_site = str(raw.get("sameSite") or "").casefold()
        mapping = {
            "strict": "Strict",
            "lax": "Lax",
            "none": "None",
            "no_restriction": "None",
        }
        if same_site in mapping:
            cookie["sameSite"] = mapping[same_site]
        cookies.append(cookie)
    return cookies


def _safe_error(error: BaseException) -> str:
    text = str(error).replace("\n", " ").strip()
    text = re.sub(r"https?://\S+", "[url]", text)
    return text[:300] or "browser operation failed"


def inspect_pdf(path: Path, extract_text: bool) -> dict:
    try:
        size = path.stat().st_size
    except OSError:
        raise EClassError("OZ PDF was not created") from None
    if size <= 0 or size > MAX_PDF_BYTES:
        raise EClassError("OZ PDF is empty or exceeds 64 MiB")
    with path.open("rb") as stream:
        if stream.read(5) != b"%PDF-":
            raise EClassError("OZ export did not return a valid PDF")
    try:
        from pypdf import PdfReader
    except ImportError:
        raise EClassError("PDF text extraction needs the pypdf package") from None
    try:
        reader = PdfReader(path)
        if reader.is_encrypted or not reader.pages:
            raise EClassError("OZ PDF is encrypted or empty")
        page_count = len(reader.pages)
        pages = [page.extract_text() or "" for page in reader.pages] if extract_text else []
    except EClassError:
        raise
    except Exception:
        raise EClassError("OZ PDF could not be parsed") from None
    return {
        "course_id": None,
        "path": str(path.resolve()),
        "bytes": size,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "content_type": "application/pdf",
        "source": "oz_native_export",
        "page_count": page_count,
        "text": "\n\n".join(pages) if extract_text else None,
        "pages": [{"page": index + 1, "text": value} for index, value in enumerate(pages)],
        "text_complete": all(value.strip() for value in pages) if extract_text else None,
    }


async def export_syllabus_pdf(
    course_id: int,
    output_dir: Path,
    session_file: Path | None = None,
    browser_path: str | None = None,
    extract_text: bool = True,
    timeout: float = DEFAULT_TIMEOUT,
) -> dict:
    session = load_session(session_file)
    if session is None:
        raise EClassError("No saved e-Class session; run `eclass-mcp login` first")
    cookies = playwright_cookies(session)
    if not cookies:
        raise EClassError("The saved session has no .cau.ac.kr cookies; run `eclass-mcp login` again")
    try:
        from playwright.async_api import async_playwright
    except ImportError:
        raise EClassError(
            "OZ PDF export needs Playwright: install the Python package with the `oz` extra"
        ) from None
    output_dir = output_dir.expanduser().resolve()
    output_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    destination = output_dir / f"syllabus-{course_id}-{uuid4().hex[:8]}.pdf"
    if destination.exists():
        raise EClassError("OZ output path already exists")
    executable = find_browser(browser_path)
    timeout_ms = max(10, timeout) * 1000
    try:
        async with async_playwright() as playwright:
            launch_options = {"headless": True}
            if executable is not None:
                launch_options["executable_path"] = str(executable)
            browser = await playwright.chromium.launch(**launch_options)
            try:
                context = await browser.new_context(
                    viewport={"width": 1440, "height": 1000},
                    accept_downloads=True,
                )
                try:
                    await context.add_cookies(cookies)
                    page = await context.new_page()
                    await page.goto(
                        f"{ORIGIN}/courses/{course_id}/assignments/syllabus",
                        wait_until="domcontentloaded",
                        timeout=timeout_ms,
                    )
                    if "/login" in page.url:
                        raise EClassError("The saved session is not logged in; run `eclass-mcp login` again")
                    frame = page.frame_locator(f'iframe[src^="{OZ_PREFIX}"]')
                    save = frame.locator('input.btnSAVEAS[title="Save"]')
                    await save.click(timeout=timeout_ms)
                    dialog = frame.get_by_role("dialog", name="Save", exact=True)
                    await dialog.locator("select.oz_ui_layout").select_option(
                        label="Adobe PDF File(*.pdf)"
                    )
                    async with page.expect_download(timeout=timeout_ms) as pending:
                        await dialog.get_by_role("button", name="OK", exact=True).click()
                    download = await pending.value
                    await download.save_as(str(destination))
                    if await download.failure():
                        raise EClassError("OZ PDF download failed")
                finally:
                    await context.close()
            finally:
                await browser.close()
    except EClassError:
        destination.unlink(missing_ok=True)
        raise
    except Exception as error:
        destination.unlink(missing_ok=True)
        raise EClassError(f"OZ browser export failed: {_safe_error(error)}") from None
    try:
        os.chmod(destination, 0o600)
        result = inspect_pdf(destination, extract_text)
        result["course_id"] = course_id
        return result
    except BaseException:
        destination.unlink(missing_ok=True)
        raise


def _output_dir(value: str | None) -> Path:
    if value:
        return Path(value).expanduser()
    configured = os.getenv("ECLASS_DOWNLOAD_DIR")
    if configured:
        return Path(configured).expanduser()
    return Path.home() / "Downloads" / "cau-eclass"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Export a CAU e-Class OZ syllabus PDF")
    parser.add_argument("--course-id", type=int, required=True, help="Canvas course ID")
    parser.add_argument("--output-dir", help="Directory for the PDF (default: ECLASS_DOWNLOAD_DIR)")
    parser.add_argument("--session-file", help="Saved session path (default: ECLASS_SESSION_FILE)")
    parser.add_argument("--browser-path", help="Chrome/Edge/Chromium executable")
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT, help="Seconds per OZ step")
    parser.add_argument("--no-extract-text", action="store_true", help="Skip pypdf text extraction")
    args = parser.parse_args(argv)
    if args.course_id <= 0:
        print("course_id must be positive", file=sys.stderr)
        return 2
    try:
        result = asyncio.run(
            export_syllabus_pdf(
                course_id=args.course_id,
                output_dir=_output_dir(args.output_dir),
                session_file=Path(args.session_file).expanduser() if args.session_file else default_session_file(),
                browser_path=args.browser_path,
                extract_text=not args.no_extract_text,
                timeout=args.timeout,
            )
        )
    except EClassError as error:
        print(str(error), file=sys.stderr)
        return 1
    except Exception:
        print("OZ PDF export failed", file=sys.stderr)
        return 1
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
