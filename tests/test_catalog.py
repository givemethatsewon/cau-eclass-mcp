import copy
import json
import unicodedata

import pytest

from eclass_mcp.transport import EClassError


@pytest.fixture
def weekly_modules():
    # Synthetic LearningX metadata: the numeric attendance ID differs from the opaque Commons ID.
    return [
        {
            "module_id": 88,
            "title": "9주차",
            "position": 9,
            "module_items": [
                {
                    "module_item_id": 99,
                    "title": "Scheduler (3)",
                    "content_type": "attendance_item",
                    "content_id": 111,
                    "content_data": {
                        "item_id": 111,
                        "course_id": 123,
                        "item_content_type": "commons",
                        "opened": True,
                        "lecture_period_status": "open",
                        "week_position": 9,
                        "created_at": "2026-01-01T00:00:00Z",
                        "updated_at": "2026-01-02T00:00:00Z",
                        "item_content_data": {
                            "content_id": "synthetic-opaque",
                            "content_type": "pdf",
                            "file_name": "Scheduler.pdf",
                            "view_url": "https://ocs.cau.ac.kr/em/synthetic-opaque",
                            "download_url": "https://invalid.example/file?token=synthetic-secret",
                            "thumbnail": "https://invalid.example/thumbnail",
                        },
                    },
                }
            ],
        }
    ]


@pytest.mark.parametrize(
    ("value", "expected"),
    [("commons:123:111", ("commons", 123, 111)), ("canvas:123:456", ("canvas", 123, 456))],
)
def test_parse_resource_id_preserves_provider_and_numeric_identity(value, expected):
    from eclass_mcp.catalog import parse_resource_id

    assert parse_resource_id(value) == expected


@pytest.mark.parametrize(
    "value",
    ["", "123", "commons:0:1", "canvas:1:-2", "commons:1:0", "commons:1:2:3",
     "canvas:1.0:2", "Canvas:1:2", " commons:1:2", "commons:1:2\n", "other:1:2", None],
)
def test_parse_resource_id_rejects_ambiguous_or_nonpositive_ids(value):
    from eclass_mcp.catalog import parse_resource_id

    with pytest.raises(EClassError):
        parse_resource_id(value)


def test_course_matches_normalizes_korean_and_filters_term_independently():
    from eclass_mcp.catalog import course_matches

    course = {
        "id": 123,
        "name": unicodedata.normalize("NFD", "운영 체제"),
        "course_code": "CS 301",
        "term": {"id": 71, "name": "2026 학년도 2 학기", "secret": "omit"},
    }
    assert course_matches(course, query="운영체제", term="2026학년도2")
    assert course_matches(course, query="cs301", term="71")
    assert not course_matches(course, query="운영체제", term="7")
    assert not course_matches(course, query="네트워크", term="71")
    assert course_matches(course)


def test_catalog_flattens_commons_pdf_without_private_metadata(weekly_modules):
    from eclass_mcp.catalog import material_catalog

    assert material_catalog(123, weekly_modules, []) == [
        {
            "resource_id": "commons:123:111",
            "course_id": 123,
            "title": "Scheduler (3)",
            "week": 9,
            "source": "commons",
            "content_type": "pdf",
            "available": True,
            "download_supported": True,
            "text_supported": True,
            "view_url": "https://ocs.cau.ac.kr/em/synthetic-opaque",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-02T00:00:00Z",
        }
    ]


def test_catalog_links_canvas_file_to_its_actual_module_week(weekly_modules):
    from eclass_mcp.catalog import material_catalog

    weekly_modules[0]["module_items"] = [{"id": 50, "title": "Week 9 notes", "type": "File", "content_id": 456}]
    files = [{
        "id": "456", "display_name": "notes.pdf", "content-type": "application/pdf", "size": 123,
        "locked_for_user": False, "created_at": "2026-01-03T00:00:00Z",
        "updated_at": "2026-01-04T00:00:00Z", "url": "https://invalid.example/?verifier=secret",
    }]
    result = material_catalog(123, weekly_modules, files)
    assert result == [{
        "resource_id": "canvas:123:456", "course_id": 123, "title": "Week 9 notes", "week": 9,
        "source": "canvas", "content_type": "pdf", "available": True,
        "download_supported": True, "text_supported": True,
        "created_at": "2026-01-03T00:00:00Z", "updated_at": "2026-01-04T00:00:00Z",
    }]
    assert "secret" not in json.dumps(result)


def test_catalog_keeps_unreferenced_canvas_files_downloadable():
    from eclass_mcp.catalog import material_catalog

    result = material_catalog(123, [], [{"id": 457, "display_name": "archive.zip", "content-type": "application/zip"}])
    assert result[0]["resource_id"] == "canvas:123:457"
    assert result[0]["week"] is None
    assert result[0]["content_type"] == "application/zip"
    assert result[0]["download_supported"] is True
    assert result[0]["text_supported"] is False


def test_catalog_deduplicates_repeated_source_id_and_preserves_first_week(weekly_modules):
    from eclass_mcp.catalog import material_catalog

    repeated = copy.deepcopy(weekly_modules[0])
    repeated["position"] = 10
    repeated["title"] = "10주차"
    repeated["module_items"][0]["content_data"]["week_position"] = 10
    result = material_catalog(123, weekly_modules + [repeated], [])
    assert len(result) == 1
    assert result[0]["week"] == 9


def test_reused_canvas_resource_remains_filterable_from_every_actual_week():
    from eclass_mcp.catalog import filter_materials, material_catalog

    modules = [
        {"module_id": 88, "title": "9주차", "position": 9,
         "module_items": [{"id": 50, "title": "Shared notes", "type": "File", "content_id": 456}]},
        {"module_id": 89, "title": "10주차", "position": 10,
         "module_items": [{"id": 51, "title": "Shared notes", "type": "File", "content_id": 456}]},
    ]
    files = [{"id": 456, "display_name": "notes.pdf", "content-type": "application/pdf"}]
    catalog = material_catalog(123, modules, files)
    assert len(catalog) == 1
    assert catalog[0]["week"] == 9
    assert catalog[0]["weeks"] == [9, 10]
    assert [item["resource_id"] for item in filter_materials(catalog, week=10)] == ["canvas:123:456"]
    assert [item["resource_id"] for item in filter_materials(catalog, week=9)] == ["canvas:123:456"]
    assert filter_materials(catalog, week=11) == []


@pytest.mark.parametrize("foreign_course_id", [999, "999"])
def test_catalog_rejects_commons_metadata_from_another_course(weekly_modules, foreign_course_id):
    from eclass_mcp.catalog import material_catalog

    weekly_modules[0]["module_items"][0]["content_data"]["course_id"] = foreign_course_id
    assert material_catalog(123, weekly_modules, []) == []


@pytest.mark.parametrize(
    ("container", "key", "value"),
    [("data", "opened", False), ("data", "lecture_period_status", "not_open"),
     ("data", "lecture_period_status", "locked"), ("data", "lecture_period_status", "before_open"),
     ("data", "lecture_period_status", "closed"), ("module", "published", False),
     ("data", "published", False), ("media", "published", False),
     ("item", "locked", True), ("data", "locked_for_user", True),
     ("media", "content_id", "not_open")],
)
def test_catalog_marks_closed_commons_resources_unavailable(weekly_modules, container, key, value):
    from eclass_mcp.catalog import material_catalog

    item = weekly_modules[0]["module_items"][0]
    target = {"module": weekly_modules[0], "item": item, "data": item["content_data"],
              "media": item["content_data"]["item_content_data"]}[container]
    target[key] = value
    resource = material_catalog(123, weekly_modules, [])[0]
    assert resource["available"] is False
    assert resource["download_supported"] is False
    assert resource["text_supported"] is False
    assert resource["unavailable_reason"]


def test_catalog_marks_locked_canvas_file_unavailable():
    from eclass_mcp.catalog import material_catalog

    resource = material_catalog(123, [], [{"id": 456, "display_name": "notes.pdf", "locked_for_user": True}])[0]
    assert resource["available"] is False
    assert resource["download_supported"] is False
    assert resource["text_supported"] is False
    assert resource["unavailable_reason"]


def test_catalog_does_not_advertise_unified_video_download(weekly_modules):
    from eclass_mcp.catalog import material_catalog

    weekly_modules[0]["module_items"][0]["content_data"]["item_content_data"]["content_type"] = "video/mp4"
    resource = material_catalog(123, weekly_modules, [])[0]
    assert resource["available"] is True
    assert resource["content_type"] == "mp4"
    assert resource["download_supported"] is False
    assert resource["text_supported"] is False


def test_catalog_skips_material_without_numeric_source_id(weekly_modules):
    from eclass_mcp.catalog import material_catalog

    item = weekly_modules[0]["module_items"][0]
    item.pop("content_id")
    item["content_data"].pop("item_id")
    assert material_catalog(123, weekly_modules, [{"display_name": "anonymous.pdf"}]) == []


@pytest.mark.parametrize("url", [
    "https://ocs.cau.ac.kr/em/synthetic?token=secret",
    "https://ocs.cau.ac.kr/em/synthetic#secret",
    "https://ocs.cau.ac.kr/download/synthetic",
    "https://ocs.cau.ac.kr/em/../download",
    "https://ocs.cau.ac.kr/em/%2e%2e/download",
    "https://secret@ocs.cau.ac.kr/em/synthetic",
    "https://invalid.example/em/synthetic",
])
def test_catalog_omits_signed_or_untrusted_viewer_urls(weekly_modules, url):
    from eclass_mcp.catalog import material_catalog

    weekly_modules[0]["module_items"][0]["content_data"]["item_content_data"]["view_url"] = url
    assert "view_url" not in material_catalog(123, weekly_modules, [])[0]


def test_material_filters_combine_real_week_normalized_title_and_pdf_mime():
    from eclass_mcp.catalog import filter_materials

    items = [
        {"resource_id": "commons:123:111", "title": unicodedata.normalize("NFD", "프로세스 스케줄링"), "week": 9, "content_type": "pdf"},
        {"resource_id": "canvas:123:456", "title": "프로세스 스케줄링", "week": 10, "content_type": "application/pdf"},
        {"resource_id": "commons:123:112", "title": "프로세스 스케줄링", "week": 9, "content_type": "mp4"},
    ]
    result = filter_materials(items, week=9, query="프로세스스케줄링", content_type="application/pdf")
    assert [item["resource_id"] for item in result] == ["commons:123:111"]
    assert [item["resource_id"] for item in filter_materials(items, content_type="pdf")] == ["commons:123:111", "canvas:123:456"]
    assert filter_materials(items, query="missing") == []


def test_compact_modules_preserve_empty_modules_and_nonfile_items(weekly_modules):
    from eclass_mcp.catalog import compact_modules

    weekly_modules.append({"module_id": 89, "title": "10주차", "position": 10, "module_items": []})
    weekly_modules[0]["module_items"].append({"module_item_id": 100, "title": "중간 과제", "content_type": "assignment", "content_id": 300})
    result = compact_modules(123, weekly_modules)
    assert [(module["module_id"], module["week"]) for module in result] == [(88, 9), (89, 10)]
    assert result[0]["items"][0]["resource_id"] == "commons:123:111"
    assert result[0]["items"][1] == {"item_id": 100, "title": "중간 과제", "content_type": "assignment", "available": True}
    assert result[1]["items"] == []
    assert "synthetic-secret" not in json.dumps(result)


def test_compact_module_filters_remove_empty_and_nonmatching_modules(weekly_modules):
    from eclass_mcp.catalog import compact_modules

    weekly_modules.append({"module_id": 89, "title": "10주차", "position": 10, "module_items": []})
    assert len(compact_modules(123, weekly_modules, week=9, query="scheduler", content_type="pdf")) == 1
    assert compact_modules(123, weekly_modules, week=10) == []
    assert compact_modules(123, weekly_modules, query="missing") == []
