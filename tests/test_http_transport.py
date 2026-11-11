"""HTTP transport contract with synthetic school responses and no network."""

from __future__ import annotations

import base64
import io
import json

import httpx
import pytest
from pypdf import PdfReader, PdfWriter

from eclass_mcp.http_transport import HTTPTransport
from eclass_mcp.session import Session, SessionExpired
from eclass_mcp.transport import ORIGIN, EClassError


def pdf_bytes():
    stream = io.BytesIO()
    writer = PdfWriter()
    writer.add_blank_page(width=100, height=100)
    writer.write(stream)
    return stream.getvalue()


def session() -> Session:
    return Session(
        cookies=[
            {"name": "xn_api_token", "value": "synthetic-token", "domain": ".cau.ac.kr", "path": "/"},
            {"name": "session", "value": "synthetic-session", "domain": "eclass3.cau.ac.kr", "path": "/"},
        ]
    )


def client(handler):
    return httpx.AsyncClient(
        transport=httpx.MockTransport(handler), cookies=session().as_cookies(), follow_redirects=False
    )


def commons_fixtures(*, primary_status=500, view=True, xml_target=None, length=None, media="pdf"):
    data = pdf_bytes() if media == "pdf" else b"\x00\x00\x00\x18ftypisom" + b"\x00" * 32
    metadata = {
        "extension": "pdf" if media == "pdf" else "mp4",
        "is_original_file_deleted": False,
        "original_file_length": len(data) if length is None else length,
        "download_url": "https://ocs.cau.ac.kr/download/fixture?signature=synthetic-private",
        "view_url": "https://ocs.cau.ac.kr/view/fixture?signature=synthetic-private" if view else None,
    }
    target = xml_target or "https://ocs.cau.ac.kr/file?file_subpath=contents%5Cweb_files%5Coriginal.pdf&amp;key=synthetic-private"
    responses = {
        ORIGIN + "/learningx/api/v1/courses/11/modules?include_detail=true": {
            "json": [
                {
                    "module_items": [
                        {
                            "content_data": {
                                "item_id": 22,
                                "course_id": 11,
                                "item_content_type": "commons",
                                "opened": True,
                                "title": "Synthetic lecture",
                                "item_content_data": {
                                    "content_id": "fixture",
                                    "content_type": "pdf" if media == "pdf" else "mp4",
                                },
                            }
                        }
                    ]
                }
            ]
        },
        ORIGIN + "/learningx/api/v1/commons/contents?content_id=fixture": {"json": {"result": metadata}},
        "https://ocs.cau.ac.kr/download/fixture?signature=synthetic-private": {
            "status": primary_status,
            "text": "unavailable" if primary_status >= 400 else "",
            "type": "text/html",
        },
    }
    if media == "pdf":
        responses["https://ocs.cau.ac.kr/view/fixture?signature=synthetic-private"] = {
            "text": "<script>var contentUri = '/content/fixture.xml?key=synthetic-private';</script>",
            "type": "text/html",
        }
        responses["https://ocs.cau.ac.kr/content/fixture.xml?key=synthetic-private"] = {
            "text": f"<content><content_download_uri>{target}</content_download_uri></content>",
            "type": "application/xml",
        }
        responses[
            "https://ocs.cau.ac.kr/file?file_subpath=contents%5Cweb_files%5Coriginal.pdf&key=synthetic-private"
        ] = {"base64": base64.b64encode(data).decode(), "type": "application/pdf"}
    else:
        responses["https://ocs.cau.ac.kr/video.mp4"] = {
            "base64": base64.b64encode(data).decode(),
            "type": "video/mp4",
        }
        metadata["download_url"] = "https://ocs.cau.ac.kr/video.mp4"
        responses[metadata["download_url"]] = {
            "status": primary_status,
            "base64": base64.b64encode(data).decode() if primary_status < 400 else "",
            "type": "video/mp4" if primary_status < 400 else "text/html",
        }
        responses["https://ocs.cau.ac.kr/view/fixture?signature=synthetic-private"] = {
            "text": "<script>var contentUri = '/content/video.xml';</script>",
            "type": "text/html",
        }
        responses["https://ocs.cau.ac.kr/content/video.xml"] = {
            "text": (
                '<media><media_uri method="progressive" target="all">'
                "https://ocs.cau.ac.kr/play/[MEDIA_FILE]</media_uri>"
                '<main_media media_id="1">fixture.mp4</main_media></media>'
            ),
            "type": "application/xml",
        }
        responses["https://ocs.cau.ac.kr/play/fixture.mp4"] = {
            "base64": base64.b64encode(data).decode(),
            "type": "video/mp4",
        }
    return responses, data


def handler_for(responses, calls):
    def handle(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        fixture = responses.get(str(request.url))
        if fixture is None:
            return httpx.Response(404, text="unexpected")
        body = (
            base64.b64decode(fixture["base64"])
            if "base64" in fixture
            else fixture.get("text", "").encode()
        )
        if "json" in fixture:
            body = json.dumps(fixture["json"]).encode()
        return httpx.Response(
            fixture.get("status", 200),
            content=body,
            headers={"content-type": fixture.get("type", "application/json")},
        )

    return handle


async def test_get_parses_canvas_prefix_and_learningx_bearer():
    calls = []

    def handle(request):
        calls.append(request)
        if request.url.path == "/api/v1/users/self/profile":
            return httpx.Response(
                200,
                text='while(1);{"id":7,"name":"Fixture"}',
                headers={"content-type": "application/json"},
            )
        return httpx.Response(
            200,
            json=[],
            headers={
                "content-type": "application/json",
                "link": f'<{ORIGIN}/api/v1/courses?page=2>; rel="next"',
            },
        )

    async with HTTPTransport(session=session(), client=client(handle)) as transport:
        result = await transport.get("/api/v1/users/self/profile")
        assert result["body"]["id"] == 7
        result = await transport.get("/learningx/api/v1/courses/1/modules", learningx=True)
        assert result["link"].endswith('rel="next"')
    assert calls[0].headers["accept"] == "application/json+canvas-string-ids"
    assert calls[1].headers["authorization"] == "Bearer synthetic-token"


async def test_auth_refresh_retries_once():
    calls = []
    refreshed = []

    class Refreshing(HTTPTransport):
        async def refresh(self):
            refreshed.append(True)
            self.session = session()
            self.session.upsert(
                {"name": "xn_api_token", "value": "new-token", "domain": ".cau.ac.kr", "path": "/"}
            )
            self._client.cookies = self.session.as_cookies()
            return self.session

    def handle(request):
        calls.append(request)
        if len(calls) == 1:
            return httpx.Response(401)
        return httpx.Response(200, json=[], headers={"content-type": "application/json"})

    async with Refreshing(session=session(), client=client(handle)) as transport:
        assert (await transport.get("/learningx/api/v1/courses/1/modules", learningx=True))["body"] == []
    assert len(calls) == 2 and refreshed == [True]
    assert calls[1].headers["authorization"] == "Bearer new-token"


async def test_login_html_is_a_session_error():
    def handle(request):
        return httpx.Response(200, text="<html>login</html>", headers={"content-type": "text/html"})

    class NoRefresh(HTTPTransport):
        async def refresh(self):
            raise EClassError("no browser")

    async with NoRefresh(session=session(), client=client(handle)) as transport:
        with pytest.raises(SessionExpired, match="login"):
            await transport.get("/api/v1/users/self/profile")


async def test_download_rejects_untrusted_redirect_and_cleans_up(tmp_path):
    def handle(request):
        if request.url.host == "eclass3.cau.ac.kr":
            return httpx.Response(302, headers={"location": "https://evil.test/file"})
        return httpx.Response(200, content=b"secret")

    target = tmp_path / "denied.pdf"
    async with HTTPTransport(session=session(), client=client(handle)) as transport:
        with pytest.raises(EClassError):
            await transport.download("/courses/1/files/2/download?wrap=1", target, 100)
    assert not target.exists()


async def test_download_limit_and_no_overwrite(tmp_path):
    response = httpx.Response(
        200, content=b"123456", headers={"content-type": "application/pdf", "content-length": "6"}
    )

    async def handle(request):
        return response

    target = tmp_path / "download.pdf"
    async with HTTPTransport(session=session(), client=client(handle)) as transport:
        with pytest.raises(EClassError, match="limit"):
            await transport.download("/courses/1/files/2/download?wrap=1", target, 4)
    assert not target.exists()

    target.write_bytes(b"keep")
    async with HTTPTransport(session=session(), client=client(handle)) as transport:
        with pytest.raises(FileExistsError):
            await transport.download("/courses/1/files/2/download?wrap=1", target, 100)
    assert target.read_bytes() == b"keep"


async def test_commons_pdf_uses_native_metadata_and_strips_signed_urls(tmp_path):
    responses, data = commons_fixtures()
    calls = []
    target = tmp_path / "lecture.pdf"
    async with HTTPTransport(
        session=session(), client=client(handler_for(responses, calls))
    ) as transport:
        result = await transport.commons_pdf(11, 22, target)
    assert target.read_bytes() == data
    assert len(PdfReader(target).pages) == 1
    assert result == {
        "available": True,
        "title": "Synthetic lecture",
        "source": "commons_original",
        "original": True,
        "expected_bytes": len(data),
        "bytes": len(data),
        "content_type": "application/pdf",
    }
    assert "synthetic-private" not in json.dumps(result)
    download_calls = [call for call in calls if call.url.host == "ocs.cau.ac.kr"]
    assert all("authorization" not in call.headers for call in download_calls)
    assert any("synthetic-token" in call.headers.get("cookie", "") for call in download_calls)
    assert all("synthetic-session" not in call.headers.get("cookie", "") for call in download_calls)


async def test_commons_pdf_rejects_untrusted_xml_target(tmp_path):
    responses, _ = commons_fixtures(xml_target="https://evil.test/original.pdf")
    target = tmp_path / "denied.pdf"
    async with HTTPTransport(
        session=session(), client=client(handler_for(responses, []))
    ) as transport:
        with pytest.raises(EClassError, match="Untrusted"):
            await transport.commons_pdf(11, 22, target)
    assert not target.exists()


async def test_commons_pdf_checks_declared_size(tmp_path):
    responses, data = commons_fixtures(length=len(pdf_bytes()) + 1)
    target = tmp_path / "truncated.pdf"
    async with HTTPTransport(
        session=session(), client=client(handler_for(responses, []))
    ) as transport:
        with pytest.raises(EClassError, match="mismatch"):
            await transport.commons_pdf(11, 22, target)
    assert not target.exists()


async def test_video_original_and_playback_fallback(tmp_path):
    responses, data = commons_fixtures(media="mp4", primary_status=200)
    target = tmp_path / "video.mp4"
    async with HTTPTransport(
        session=session(), client=client(handler_for(responses, []))
    ) as transport:
        result = await transport.video_original(11, 22, target)
    assert target.read_bytes() == data
    assert result["source"] == "commons_original" and result["bytes"] == len(data)

    responses, data = commons_fixtures(media="mp4")
    target2 = tmp_path / "fallback.mp4"
    async with HTTPTransport(
        session=session(), client=client(handler_for(responses, []))
    ) as transport:
        with pytest.raises(EClassError, match="playback"):
            await transport.video_original(11, 22, target2)
    assert not target2.exists()

    async with HTTPTransport(
        session=session(), client=client(handler_for(responses, []))
    ) as transport:
        result = await transport.video_original(11, 22, target2, allow_playback_copy=True)
    assert target2.read_bytes() == data
    assert result["source"] == "commons_playback_mp4" and result["original"] is False


def test_from_env_requires_saved_session(tmp_path, monkeypatch):
    monkeypatch.setenv("ECLASS_SESSION_FILE", str(tmp_path / "missing.json"))
    with pytest.raises(EClassError, match="login"):
        HTTPTransport.from_env()
