import base64
import json

import pytest
from fastmcp import Client

from eclass_mcp.server import create_server
from eclass_mcp.service import EClassService, clean, date_fields
from eclass_mcp.transport import EClassError, local_path


class FakeTransport:
    def __init__(self):
        self.calls = []
        self.writes = 0
        self.attempt = 0
        self.locked = False
        self.fail_write = False

    async def get(self, path, learningx=False, html=False):
        self.calls.append(path)
        if path == "/api/v1/users/self/profile":
            body = {"id": 7, "name": "Test"}
        elif "/submissions/self" in path:
            body = {
                "attempt": self.attempt,
                "submitted_at": "2026-10-26T11:00:00Z" if self.attempt else None,
                "attachments": [{"id": 99}] if self.attempt else [],
            }
        elif "/assignments/2?" in path:
            body = {
                "id": 2,
                "name": "Project",
                "submission_types": ["online_upload"],
                "locked_for_user": self.locked,
                "allowed_extensions": ["pdf"],
            }
        elif path.startswith("/api/v1/courses?"):
            body = [{"id": 1, "name": "Course"}]
        elif path.startswith("/learningx/"):
            body = []
        else:
            body = []
        return {"status": 200, "body": body, "link": None}

    async def submit_files(self, plan):
        self.writes += 1
        if self.fail_write:
            raise TimeoutError()
        self.attempt += 1
        return {"file_ids": [99]}


async def test_mcp_schema_and_disabled_submission():
    t = FakeTransport()
    async with Client(create_server(EClassService(t))) as client:
        tools = await client.list_tools()
        names = {x.name for x in tools}
        assert len(names) == 21
        assert {"get_syllabus", "prepare_submission", "submit_assignment"} <= names
        result = await client.call_tool("list_courses", {})
        assert result.data["result"]["items"][0]["name"] == "Course"
        with pytest.raises(Exception, match="disabled"):
            await client.call_tool("submit_assignment", {"plan_id": "anything", "confirmation": "SUBMIT 2"})
        with pytest.raises(Exception, match="greater than"):
            await client.call_tool("get_assignment", {"course_id": -1, "assignment_id": 2})
    assert t.writes == 0


async def test_prepare_does_not_upload(tmp_path):
    t = FakeTransport()
    svc = EClassService(t)
    f = tmp_path / "work.pdf"
    f.write_bytes(b"%PDF-1.7 test")
    plan = await svc.prepare(1, 2, [str(f)])
    assert plan["ready"] and not plan["submission_enabled"]
    assert t.writes == 0
    assert all("/files" not in c for c in t.calls)


async def test_changed_file_blocks_submission(tmp_path):
    t = FakeTransport()
    svc = EClassService(t, enable_submission=True)
    f = tmp_path / "work.pdf"
    f.write_bytes(b"%PDF initial")
    plan = await svc.prepare(1, 2, [str(f)])
    f.write_bytes(b"%PDF changed")
    with pytest.raises(EClassError, match="changed"):
        await svc.submit(plan["plan_id"], "SUBMIT 2")
    assert t.writes == 0


async def test_state_change_blocks_submission(tmp_path):
    t = FakeTransport()
    svc = EClassService(t, enable_submission=True)
    f = tmp_path / "work.pdf"
    f.write_bytes(b"%PDF")
    p = await svc.prepare(1, 2, [str(f)])
    t.locked = True
    with pytest.raises(EClassError, match="changed"):
        await svc.submit(p["plan_id"], "SUBMIT 2")
    assert t.writes == 0


async def test_write_readback_and_no_retry(tmp_path):
    t = FakeTransport()
    svc = EClassService(t, enable_submission=True)
    f = tmp_path / "work.pdf"
    f.write_bytes(b"%PDF")
    p = await svc.prepare(1, 2, [str(f)])
    result = await svc.submit(p["plan_id"], "SUBMIT 2")
    assert result["state"] == "verified" and t.writes == 1
    with pytest.raises(EClassError):
        await svc.submit(p["plan_id"], "SUBMIT 2")
    assert t.writes == 1


async def test_ambiguous_write_no_retry(tmp_path):
    t = FakeTransport()
    t.fail_write = True
    svc = EClassService(t, enable_submission=True)
    f = tmp_path / "work.pdf"
    f.write_bytes(b"%PDF")
    p = await svc.prepare(1, 2, [str(f)])
    with pytest.raises(EClassError, match="uncertain"):
        await svc.submit(p["plan_id"], "SUBMIT 2")
    with pytest.raises(EClassError):
        await svc.submit(p["plan_id"], "SUBMIT 2")
    assert t.writes == 1


async def test_pagination_and_cross_origin():
    class Paged:
        async def get(self, path, **kw):
            if "page=2" not in path:
                return {"body": [1], "link": '<https://eclass3.cau.ac.kr/api/v1/courses?page=2>; rel="next"'}
            return {"body": [2], "link": None}

    svc = EClassService(Paged())
    assert (await svc.paged("/api/v1/courses"))["items"] == [1, 2]
    assert not (await svc.paged("/api/v1/courses", max_pages=1))["complete"]

    class Evil:
        async def get(self, *a, **kw):
            return {"body": [], "link": '<https://example.com/steal>; rel="next"'}

    with pytest.raises(EClassError, match="Cross-origin"):
        await EClassService(Evil()).paged("/api/v1/courses")


def test_dates_redaction_and_paths():
    assert date_fields({"due_at": "2026-10-25T14:59:00Z"})["due_at_seoul"] == "2026-10-25T23:59:00+09:00"
    value = clean({"access_token": "secret", "url": "https://eclass3.cau.ac.kr/x?verifier=secret&wrap=1"})
    assert "secret" not in json.dumps(value) and "wrap=1" in value["url"]
    for path in ["//example.com", "https://example.com", "/a/../b", "/%2e%2e/b"]:
        with pytest.raises(EClassError):
            local_path(path)


async def test_concurrent_submit_only_writes_once(tmp_path):
    import asyncio

    t = FakeTransport()
    svc = EClassService(t, enable_submission=True)
    f = tmp_path / "work.pdf"
    f.write_bytes(b"%PDF")
    plan = await svc.prepare(1, 2, [str(f)])
    results = await asyncio.gather(
        svc.submit(plan["plan_id"], "SUBMIT 2"),
        svc.submit(plan["plan_id"], "SUBMIT 2"),
        return_exceptions=True,
    )
    assert t.writes == 1
    assert sum(isinstance(r, EClassError) for r in results) == 1


async def test_board_pagination_metadata():
    class Boards:
        async def get(self, *args, **kwargs):
            return {"body": {"items": [], "pagination": {"current_page": 1, "last_page": 1, "total": 0}}}

    result = await EClassService(Boards()).board_posts(1, 2, 1)
    assert result["complete"] is True
    assert result["has_more"] is False


def test_secure_params_and_signed_urls_are_redacted():
    result = clean(
        {"secure_params": "PRIVATE", "url": "https://example.com/file?X-Amz-Credential=PRIVATE&key=PRIVATE"}
    )
    assert "PRIVATE" not in json.dumps(result)


async def test_syllabus_pdf_blank_text_is_incomplete(tmp_path):
    from pypdf import PdfWriter

    class PDFTransport:
        async def syllabus_pdf(self, course, destination):
            writer = PdfWriter()
            writer.add_blank_page(width=100, height=100)
            writer.write(destination)

    result = await EClassService(PDFTransport(), output_dir=tmp_path).syllabus_document(1)
    assert result["page_count"] == 1
    assert result["text_complete"] is False
    assert result["source"] == "oz_native_export"


async def test_syllabus_invalid_pdf_removed(tmp_path):
    class HTMLTransport:
        async def syllabus_pdf(self, course, destination):
            destination.write_bytes(b"<html>Login required</html>")

    with pytest.raises(EClassError, match="validation"):
        await EClassService(HTMLTransport(), output_dir=tmp_path).syllabus_document(1)
    assert not list(tmp_path.iterdir())


async def test_video_locked_and_invalid_file(tmp_path):
    class VideoTransport:
        async def video_original(self, course, item, destination, allow_playback_copy):
            if item == 1:
                return {"available": False, "reason": "Video is not open"}
            destination.write_bytes(b"<html>error</html>")
            return {"available": True, "source": "commons_original"}

    svc = EClassService(VideoTransport(), output_dir=tmp_path)
    with pytest.raises(EClassError, match="not open"):
        await svc.download_video(1, 1)
    with pytest.raises(EClassError, match="not an MP4"):
        await svc.download_video(1, 2)
    assert not list(tmp_path.iterdir())


async def test_video_playback_copy_is_labeled(tmp_path):
    class VideoTransport:
        async def video_original(self, course, item, destination, allow_playback_copy):
            assert allow_playback_copy is True
            destination.write_bytes(b"\x00\x00\x00\x18ftypisom" + b"\x00" * 12)
            return {"available": True, "source": "commons_playback_mp4"}

    result = await EClassService(VideoTransport(), output_dir=tmp_path).download_video(1, 2, True)
    assert result["original"] is False
    assert result["source"] == "commons_playback_mp4"


@pytest.mark.parametrize("change_stage", ["during_revalidation", "after_revalidation"])
async def test_file_changed_during_submission_revalidation_is_not_uploaded(tmp_path, change_stage):
    source = tmp_path / "work.pdf"
    source.write_bytes(b"%PDF approved")
    transport = FakeTransport()

    class ChangingService(EClassService):
        change = False

        async def prepare(self, *args):
            if self.change and change_stage == "during_revalidation":
                source.write_bytes(b"%PDF modified")
            result = await super().prepare(*args)
            if self.change and change_stage == "after_revalidation":
                source.write_bytes(b"%PDF modified")
            return result

    service = ChangingService(transport, enable_submission=True)
    plan = await service.prepare(1, 2, [str(source)])
    service.change = True
    with pytest.raises(EClassError, match="changed"):
        await service.submit(plan["plan_id"], "SUBMIT 2")
    assert transport.writes == 0
    assert plan["state"] == "prepared"


async def test_submission_uses_approved_snapshot_when_file_changes_before_transport_write(tmp_path):
    source = tmp_path / "work.pdf"
    approved = b"%PDF approved"
    source.write_bytes(approved)

    class ChangingTransport(FakeTransport):
        async def submit_files(self, payload):
            source.write_bytes(b"%PDF modified")
            assert base64.b64decode(payload["files"][0]["data"]) == approved
            assert "path" not in payload["files"][0]
            return await super().submit_files(payload)

    transport = ChangingTransport()
    service = EClassService(transport, enable_submission=True)
    plan = await service.prepare(1, 2, [str(source)])
    result = await service.submit(plan["plan_id"], "SUBMIT 2")
    assert result["state"] == "verified" and transport.writes == 1


async def test_restarted_service_requires_new_submission_plan(tmp_path):
    source = tmp_path / "work.pdf"
    source.write_bytes(b"%PDF approved")
    transport = FakeTransport()
    original = EClassService(transport)
    old = await original.prepare(1, 2, [str(source)])
    restarted = EClassService(transport, enable_submission=True)
    with pytest.raises(EClassError, match="ready, unused"):
        await restarted.submit(old["plan_id"], "SUBMIT 2")
    assert transport.writes == 0
    fresh = await restarted.prepare(1, 2, [str(source)])
    assert fresh["ready"] and fresh["submission_enabled"]
