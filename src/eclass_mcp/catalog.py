"""Pure, compact views of Canvas files and LearningX weekly materials."""

from __future__ import annotations

import mimetypes
import re
import unicodedata
from urllib.parse import urlsplit

from .transport import EClassError


def _normalized(value: str | None) -> str:
    return "".join(unicodedata.normalize("NFC", value or "").casefold().split())


def _positive_id(value) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value if value > 0 else None
    if isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*", value):
        return int(value)
    return None


def parse_resource_id(value: str) -> tuple[str, int, int]:
    """Parse a provider-qualified ID without guessing an integer's source."""
    match = re.fullmatch(r"(commons|canvas):([1-9][0-9]*):([1-9][0-9]*)", value) if isinstance(value, str) else None
    if not match:
        raise EClassError("Use a resource ID such as commons:123:456 or canvas:123:456")
    return match[1], int(match[2]), int(match[3])


def course_matches(course: dict, query: str | None = None, term: str | None = None) -> bool:
    """Match course titles/codes and a term ID or term-name fragment."""
    search = _normalized(query)
    if search and not any(search in _normalized(course.get(key)) for key in ("name", "course_code")):
        return False
    term_search = _normalized(term)
    if not term_search:
        return True
    course_term = course.get("term") or {}
    if isinstance(course_term, dict):
        term_id = course_term.get("id", course.get("enrollment_term_id"))
        term_name = course_term.get("name")
    else:
        term_id, term_name = course.get("enrollment_term_id"), str(course_term)
    return term_search == str(term_id) or term_search in _normalized(term_name)


def _content_type(value: str | None, filename: str | None = None) -> str:
    value = (value or "").strip().casefold().split(";", 1)[0]
    if not value or value in {"file", "application/octet-stream"}:
        guessed = mimetypes.guess_type(filename or "")[0]
        value = guessed or value or "file"
    return {"application/pdf": "pdf", "video/mp4": "mp4"}.get(value, value)


def _data(item: dict) -> tuple[dict, dict]:
    data = item.get("content_data")
    data = data if isinstance(data, dict) else {}
    media = data.get("item_content_data")
    return data, media if isinstance(media, dict) else {}


def _week(module: dict, data: dict | None = None) -> int | None:
    for obj in (data or {}, module):
        for key in ("week_position", "week", "position"):
            if (value := _positive_id(obj.get(key))) is not None:
                return value
    match = re.search(r"([0-9]+)\s*주(?:차)?", module.get("title") or module.get("name") or "")
    return _positive_id(match[1]) if match else None


def _unavailable_reason(*objects: dict) -> str | None:
    for obj in objects:
        if obj.get("opened") is False or obj.get("published") is False:
            return "This material is not open"
        if obj.get("locked") is True or obj.get("locked_for_user") is True:
            return "This material is locked"
        if (
            obj.get("lecture_period_status") in {"not_open", "locked", "before_open", "closed"}
            or obj.get("content_id") == "not_open"
        ):
            return "This material is not open"
    return None


def _viewer_url(value) -> str | None:
    if not isinstance(value, str):
        return None
    try:
        url = urlsplit(value)
        if (
            url.scheme == "https" and url.netloc == "ocs.cau.ac.kr"
            and re.fullmatch(r"/em/[A-Za-z0-9_-]+", url.path)
            and not url.query and not url.fragment
        ):
            return value
    except ValueError:
        pass
    return None


def _commons_resource(course_id: int, module: dict, item: dict) -> dict | None:
    data, media = _data(item)
    if (data.get("item_content_type") or item.get("content_type") or "").casefold() != "commons":
        return None
    if "course_id" in data and _positive_id(data["course_id"]) != course_id:
        return None
    source_id = _positive_id(data.get("item_id")) or _positive_id(item.get("content_id"))
    if source_id is None:
        return None
    kind = _content_type(media.get("content_type"), media.get("file_name"))
    reason = _unavailable_reason(module, item, data, media)
    resource = {
        "resource_id": f"commons:{course_id}:{source_id}",
        "course_id": course_id,
        "title": item.get("title") or media.get("file_name") or data.get("title") or "Untitled material",
        "week": _week(module, data),
        "source": "commons",
        "content_type": kind,
        "available": reason is None,
        "download_supported": reason is None and kind == "pdf",
        "text_supported": reason is None and kind == "pdf",
        "created_at": data.get("created_at", item.get("created_at")),
        "updated_at": data.get("updated_at", item.get("updated_at")),
    }
    if url := _viewer_url(media.get("view_url")):
        resource["view_url"] = url
    if reason:
        resource["unavailable_reason"] = reason
    return resource


def _canvas_resource(course_id: int, module: dict, item: dict, file: dict | None = None) -> dict | None:
    file = file or {}
    source_id = _positive_id(item.get("content_id")) or _positive_id(file.get("id"))
    if source_id is None:
        return None
    data, media = _data(item)
    filename = file.get("display_name") or file.get("filename") or item.get("title")
    kind = _content_type(file.get("content-type") or file.get("content_type") or media.get("content_type"), filename)
    reason = _unavailable_reason(module, item, data, media, file)
    resource = {
        "resource_id": f"canvas:{course_id}:{source_id}",
        "course_id": course_id,
        "title": item.get("title") or filename or "Untitled file",
        "week": _week(module, data),
        "source": "canvas",
        "content_type": kind,
        "available": reason is None,
        "download_supported": reason is None,
        "text_supported": reason is None and kind == "pdf",
        "created_at": file.get("created_at", item.get("created_at")),
        "updated_at": file.get("updated_at", item.get("updated_at")),
    }
    if reason:
        resource["unavailable_reason"] = reason
    return resource


def _module_items(module: dict) -> list:
    return module.get("module_items") or module.get("items") or []


def _is_canvas_file(item: dict) -> bool:
    return (item.get("type") or item.get("content_type") or "").casefold() == "file"


def material_catalog(course_id: int, modules: list, files: list) -> list[dict]:
    """Flatten sources into stable IDs and retain every observed week."""
    by_file_id = {_positive_id(file.get("id")): file for file in files if isinstance(file, dict)}
    resources = {}

    def add(resource):
        if not resource:
            return
        prior = resources.setdefault(resource["resource_id"], resource)
        weeks = list(prior.get("weeks", [prior["week"]] if prior["week"] is not None else []))
        if resource["week"] is not None and resource["week"] not in weeks:
            weeks.append(resource["week"])
        if weeks and weeks != [prior["week"]]:
            prior["weeks"] = weeks

    for module in modules:
        for item in _module_items(module):
            resource = _commons_resource(course_id, module, item)
            if resource is None and _is_canvas_file(item):
                resource = _canvas_resource(course_id, module, item, by_file_id.get(_positive_id(item.get("content_id"))))
            add(resource)
    for file in files:
        if isinstance(file, dict) and (resource := _canvas_resource(course_id, {}, {}, file)):
            add(resource)
    return list(resources.values())


def filter_materials(
    items: list[dict], week: int | None = None, query: str | None = None, content_type: str | None = None
) -> list[dict]:
    search, kind = _normalized(query), _content_type(content_type) if content_type else None
    return [
        item for item in items
        if (week is None or item.get("week") == week or week in item.get("weeks", []))
        and (not search or search in _normalized(item.get("title")))
        and (kind is None or _content_type(item.get("content_type")) == kind)
    ]


def compact_modules(
    course_id: int, modules: list, week: int | None = None,
    query: str | None = None, content_type: str | None = None,
) -> list[dict]:
    """Keep readable weekly structure without raw nested provider metadata."""
    result = []
    filtered = week is not None or bool(_normalized(query)) or bool(content_type)
    for module in modules:
        module_week, items = _week(module), []
        for item in _module_items(module):
            resource = _commons_resource(course_id, module, item)
            if resource is None and _is_canvas_file(item):
                resource = _canvas_resource(course_id, module, item)
            if resource is None:
                data, media = _data(item)
                resource = {
                    "item_id": item.get("module_item_id", item.get("id")),
                    "title": item.get("title") or "Untitled item",
                    "content_type": item.get("content_type") or item.get("type") or "unknown",
                    "available": _unavailable_reason(module, item, data, media) is None,
                }
            if week is not None and resource.get("week", module_week) != week:
                continue
            if filter_materials([resource], query=query, content_type=content_type):
                items.append(resource)
        if not filtered or items:
            result.append({
                "module_id": module.get("module_id", module.get("id")),
                "title": module.get("title") or module.get("name") or "Untitled module",
                "week": module_week,
                "items": items,
            })
    return result
