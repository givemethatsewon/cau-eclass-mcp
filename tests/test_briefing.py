"""Briefing aggregation and partial-failure reporting."""

from __future__ import annotations

from urllib.parse import parse_qs, urlsplit

import pytest
from fastmcp import Client

from eclass_mcp.server import create_server
from eclass_mcp.service import EClassService
from eclass_mcp.transport import EClassError


class BriefingTransport:
    async def get(self, path, learningx=False, html=False):
        parsed = urlsplit(path)
        if path.startswith("/api/v1/courses?"):
            return {
                "body": [
                    {"id": 1, "name": "Algorithms", "term": {"id": 96, "name": "2026 Fall"}},
                    {"id": 2, "name": "Networks", "term": {"id": 96, "name": "2026 Fall"}},
                    {"id": 3, "name": "Graphics", "term": {"id": 96, "name": "2026 Fall"}},
                ],
                "link": None,
            }
        if path.startswith("/api/v1/users/self/todo"):
            return {
                "body": [
                    {
                        "type": "submitting",
                        "course_id": 1,
                        "context_name": "Algorithms",
                        "assignment": {
                            "id": 10,
                            "name": "Problem Set 4",
                            "due_at": "2026-11-20T14:59:00Z",
                            "points_possible": 10,
                        },
                    },
                    {
                        "type": "submitting",
                        "course_id": 2,
                        "context_name": "Networks",
                        "quiz": {
                            "id": 20,
                            "title": "Quiz 3",
                            "due_at": "2026-11-18T14:59:00Z",
                        },
                    },
                    {"type": "submitting", "course_id": 3, "context_name": "Graphics"},
                ],
                "link": None,
            }
        if parsed.path == "/api/v1/announcements":
            query = parse_qs(parsed.query)
            course_id = int(query["context_codes[]"][0].removeprefix("course_"))
            if course_id == 2:
                raise EClassError("synthetic announcement failure")
            return {
                "body": [
                    {
                        "id": 100 + course_id,
                        "title": f"Notice {course_id}",
                        "posted_at": "2026-11-11T09:00:00Z",
                        "read_state": "unread",
                        "author": {"display_name": "Instructor"},
                    }
                ],
                "link": None,
            }
        raise AssertionError(f"unexpected path: {path}")


async def test_briefing_combines_courses_deadlines_and_notices():
    result = await EClassService(BriefingTransport()).briefing(days=14, course_limit=2)
    assert [course["name"] for course in result["courses"]] == ["Algorithms", "Networks"]
    assert [item["title"] for item in result["deadlines"]] == ["Quiz 3", "Problem Set 4"]
    assert result["deadlines"][0]["due_at_seoul"] == "2026-11-18T23:59:00+09:00"
    assert result["announcements"][0]["course_name"] == "Algorithms"
    assert result["counts"] == {"courses": 2, "deadlines": 2, "announcements": 1}
    assert result["complete"] is False
    assert any("Course 2" in warning for warning in result["warnings"])


async def test_briefing_rejects_invalid_range():
    service = EClassService(BriefingTransport())
    with pytest.raises(EClassError, match="days"):
        await service.briefing(days=0)
    with pytest.raises(EClassError, match="course_limit"):
        await service.briefing(course_limit=31)


async def test_mcp_exposes_briefing_tools():
    async with Client(create_server(EClassService(BriefingTransport()))) as client:
        result = await client.call_tool("get_briefing", {"days": 14, "course_limit": 2})
        assert result.data["result"]["counts"]["deadlines"] == 2
        todo = await client.call_tool("list_todo", {})
        assert len(todo.data["result"]["items"]) == 3
