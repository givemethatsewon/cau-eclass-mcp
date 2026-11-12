from __future__ import annotations

import time

import pytest
from pypdf import PdfWriter

from eclass_mcp.errors import EClassError
from eclass_mcp.oz_export import _safe_error, find_browser, inspect_pdf, playwright_cookies
from eclass_mcp.session import Session


def test_playwright_cookies_normalize_and_filter():
    cookies = playwright_cookies(
        Session(
            cookies=[
                {
                    "name": "sid",
                    "value": "x",
                    "domain": ".cau.ac.kr",
                    "path": "/",
                    "secure": True,
                    "httpOnly": True,
                    "sameSite": "no_restriction",
                    "expires": time.time() + 600,
                },
                {"name": "other", "value": "x", "domain": ".example.com", "path": "/"},
                {
                    "name": "old",
                    "value": "x",
                    "domain": ".cau.ac.kr",
                    "path": "/",
                    "expires": time.time() - 10,
                },
            ]
        )
    )
    assert len(cookies) == 1
    cookie = cookies[0]
    assert cookie["name"] == "sid"
    assert cookie["sameSite"] == "None"
    assert 590 < cookie["expires"] - time.time() <= 600


def test_inspect_pdf_reads_pages(tmp_path):
    path = tmp_path / "blank.pdf"
    writer = PdfWriter()
    writer.add_blank_page(width=612, height=792)
    with path.open("wb") as stream:
        writer.write(stream)
    result = inspect_pdf(path, extract_text=True)
    assert result["page_count"] == 1
    assert result["pages"] == [{"page": 1, "text": ""}]
    assert result["text_complete"] is False
    assert len(result["sha256"]) == 64
    only_pdf = inspect_pdf(path, extract_text=False)
    assert only_pdf["page_count"] == 1
    assert only_pdf["pages"] == []
    assert only_pdf["text"] is None


def test_inspect_pdf_rejects_non_pdf(tmp_path):
    path = tmp_path / "fake.pdf"
    path.write_bytes(b"not a pdf")
    with pytest.raises(EClassError):
        inspect_pdf(path, extract_text=False)


def test_find_browser_accepts_explicit_file(tmp_path):
    browser = tmp_path / "chrome"
    browser.write_text("")
    assert find_browser(str(browser)) == browser


def test_safe_error_hides_urls():
    assert "[url]" in _safe_error(RuntimeError("failed https://example.com/private?token=x"))
