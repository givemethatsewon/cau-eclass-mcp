"""Validate downloaded PDFs and return resumable, bounded page text."""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from pathlib import Path

from pypdf import PdfReader

from .transport import EClassError

MAX_DOCUMENT_BYTES = 64 * 1024 * 1024


def inspect_pdf(path: Path) -> dict:
    try:
        size = path.stat().st_size
        if not 0 < size <= MAX_DOCUMENT_BYTES:
            raise ValueError("PDF size")
        with path.open("rb") as stream:
            if stream.read(5) != b"%PDF-":
                raise ValueError("PDF signature")
            stream.seek(0)
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        with PdfReader(path) as reader:
            if reader.is_encrypted or not len(reader.pages):
                raise ValueError("PDF pages")
            page_count = len(reader.pages)
    except Exception:
        raise EClassError("Downloaded PDF validation failed (invalid, encrypted, empty, or oversized)") from None
    return {"bytes": size, "sha256": digest, "page_count": page_count, "content_type": "application/pdf"}


def read_pdf(
    path: Path, start_page: int = 1, end_page: int | None = None,
    start_offset: int = 0, max_chars: int = 24000,
    sanitize: Callable[[str], str] | None = None,
) -> dict:
    if start_page < 1 or end_page is not None and end_page < start_page:
        raise EClassError("Invalid page range")
    if start_offset < 0:
        raise EClassError("Invalid text offset")
    if not 1 <= max_chars <= 100000:
        raise EClassError("max_chars must be between 1 and 100000")
    pages, blank, failed = [], [], []
    next_page, next_offset = None, 0
    try:
        reader = PdfReader(path)
    except Exception:
        raise EClassError("PDF could not be opened") from None
    try:
        count = len(reader.pages)
        stop = min(count, start_page + 9) if end_page is None else end_page
        if start_page > count or stop > count:
            raise EClassError(f"Requested page range exceeds PDF page count ({count})")
        remaining = max_chars
        for number in range(start_page, stop + 1):
            try:
                value = reader.pages[number - 1].extract_text() or ""
                if sanitize is not None:
                    value = sanitize(value)
            except Exception:
                value = ""
                failed.append(number)
            offset = start_offset if number == start_page else 0
            if offset > len(value):
                raise EClassError("Text offset exceeds extracted page length")
            if not value.strip():
                blank.append(number)
            chunk = value[offset:offset + remaining]
            consumed = offset + len(chunk)
            pages.append({"page": number, "text": chunk, "start_offset": offset,
                          "end_offset": consumed, "total_chars": len(value)})
            remaining -= len(chunk)
            if consumed < len(value):
                next_page, next_offset = number, consumed
                break
            if not remaining and number < stop:
                next_page = number + 1
                break
        range_complete = next_page is None
        if range_complete and stop < count:
            next_page = stop + 1
        warnings = []
        if blank:
            warnings.append("Some pages have no extracted text; diagrams/scans are not OCRed")
        if failed:
            warnings.append("Text extraction failed on some pages")
        return {
            "page_count": count, "start_page": start_page, "end_page": stop,
            "pages": pages, "pages_without_text": blank, "text_error_pages": failed,
            "range_complete": range_complete,
            "text_complete": range_complete and not blank and not failed and start_offset == 0,
            "document_complete": (
                start_page == 1 and stop == count and range_complete
                and start_offset == 0 and not blank and not failed
            ),
            "has_more": next_page is not None, "next_page": next_page, "next_offset": next_offset,
            "warnings": warnings,
        }
    finally:
        reader.close()
