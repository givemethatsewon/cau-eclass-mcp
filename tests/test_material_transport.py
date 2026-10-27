"""Synthetic Commons files exercise real metadata, transfer, and privacy boundaries."""

import base64
import io
import json
import os
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

import httpx
import pytest
from pypdf import PdfReader, PdfWriter

from eclass_mcp.cdp import CDPTransport
from eclass_mcp.transport import ORIGIN, AsideTransport, EClassError


@pytest.fixture
def pdf_bytes():
    stream = io.BytesIO()
    writer = PdfWriter()
    writer.add_blank_page(width=100, height=100)
    writer.write(stream)
    return stream.getvalue()


def fixtures(pdf_bytes, *, item_changes=None, meta_changes=None, redirect=None):
    item = {
        "item_id": 22,
        "course_id": 11,
        "item_content_type": "commons",
        "opened": True,
        "lecture_period_status": "open",
        "title": "Synthetic lecture",
        "item_content_data": {"content_id": "fixture", "content_type": "pdf"},
    }
    item.update(item_changes or {})
    metadata = {
        "extension": "pdf",
        "is_original_file_deleted": False,
        "original_file_length": len(pdf_bytes),
        "download_url": "https://ocs.cau.ac.kr/download/fixture?signature=synthetic-private",
        "view_url": "https://ocs.cau.ac.kr/view/fixture?signature=synthetic-private",
        "author": "Private author",
    }
    metadata.update(meta_changes or {})
    responses = {
        ORIGIN + "/learningx/api/v1/courses/11/modules?include_detail=true": {
            "json": [{"module_items": [{"content_data": item}]}],
        },
        ORIGIN + "/learningx/api/v1/commons/contents?content_id=fixture": {
            "json": {"result": metadata},
        },
        metadata["download_url"]: {"status": 500, "text": "unavailable", "type": "text/html"},
        metadata["view_url"]: {
            "text": "<script>var contentUri = '/content/fixture.xml?key=synthetic-private';</script>",
            "type": "text/html",
        },
        "https://ocs.cau.ac.kr/content/fixture.xml?key=synthetic-private": {
            "text": "<content><content_download_uri>/file?file_subpath=contents%5Cweb_files%5Coriginal.pdf&amp;key=synthetic-private</content_download_uri></content>",
            "type": "application/xml",
        },
        "https://ocs.cau.ac.kr/file?file_subpath=contents%5Cweb_files%5Coriginal.pdf&key=synthetic-private": {
            "base64": base64.b64encode(pdf_bytes).decode(),
            "type": "application/octet-stream",
        },
    }
    if redirect:
        responses[metadata["download_url"]] = {"status": 302, "headers": {"location": redirect}}
        responses[redirect] = {"base64": base64.b64encode(pdf_bytes).decode(), "type": "application/pdf"}
    return responses


def node_result(code, responses, working_dir):
    node = shutil.which("node")
    if not node:
        pytest.skip("Node.js is required to execute the shared browser scripts")
    setup = """
import * as fs from 'node:fs/promises';
import path from 'node:path';
const responses = RESPONSES;
const pwd = WORKING_DIR;
const document = {cookie:'xn_api_token=synthetic-private'};
const calls = [];
const fetch = async (input, options={}) => {
  const url = new URL(input, 'https://eclass3.cau.ac.kr').href;
  calls.push({url,method:options.method||'GET'});
  if (!responses[url]) throw Error('Unexpected synthetic request');
  const r=responses[url];
  const headers={...(r.headers||{}),'content-type':r.type||'application/json'};
  const body=r.base64 ? Buffer.from(r.base64,'base64') : r.json ? JSON.stringify(r.json) : r.text||'';
  if((r.status||200)>=300&&(r.status||200)<400&&options.redirect!=='manual') {
    if(options.redirect==='error')throw Error('redirect rejected');
    return fetch(new URL(headers.location,url).href,options);
  }
  const response=new Response(body,{status:r.status||200,headers});
  Object.defineProperty(response,'url',{value:url});
  return response;
};
const fixturePage={evaluate:async(fn,q)=>fn(q)};
const listBrowserTabs=async()=>[{url:'https://eclass3.cau.ac.kr/courses/11',targetId:'fixture'}];
const attachBrowserTab=async()=>fixturePage;
""".replace("RESPONSES", json.dumps(responses)).replace("WORKING_DIR", json.dumps(str(working_dir)))
    result = subprocess.run(
        [node, "--input-type=module", "-"],
        input=setup + "\n" + code,
        text=True,
        capture_output=True,
        check=True,
    )
    marker = "ECLASS_RESULT:"
    output = next(line.split(marker, 1)[1] for line in result.stdout.splitlines() if marker in line)
    return json.loads(output)


@pytest.fixture(params=["aside", "cdp"])
def material_transport(request, tmp_path, monkeypatch):
    def create(responses):
        if request.param == "aside":
            class SyntheticAside(AsideTransport):
                async def run(self, code):
                    value = node_result(code, responses, tmp_path / ".aside")
                    if value.get("bridge_error"):
                        raise EClassError(value["bridge_error"])
                    return value

            monkeypatch.setattr(Path, "home", staticmethod(lambda: tmp_path))
            return SyntheticAside()

        class Page:
            url = ORIGIN + "/courses/11"

            async def evaluate(self, script, payload):
                code = "console.log('ECLASS_RESULT:'+JSON.stringify(await (" + script + ")(" + json.dumps(payload) + ")));"
                return node_result(code, responses, tmp_path)

        class Context:
            pages = [Page()]

            async def cookies(self, urls):
                return []

        page = Context.pages[0]
        page.context = Context()
        transport = CDPTransport()
        transport.browser = SimpleNamespace(is_connected=lambda: True, contexts=[page.context])
        real_client = httpx.AsyncClient

        def handle(req):
            fixture = responses.get(str(req.url))
            assert fixture is not None, "Unexpected synthetic request"
            assert req.method == "GET"
            body = base64.b64decode(fixture["base64"]) if "base64" in fixture else fixture.get("text", "").encode()
            return httpx.Response(
                fixture.get("status", 200),
                content=body,
                headers={"content-type": fixture.get("type", "application/json"), **fixture.get("headers", {})},
            )

        monkeypatch.setattr(httpx, "AsyncClient", lambda **kw: real_client(transport=httpx.MockTransport(handle), **kw))
        return transport

    return create


async def test_commons_download_uses_provided_native_uri_without_exposing_it(material_transport, tmp_path, pdf_bytes):
    # Break caught: failing primary endpoints no longer resolve the provided XML URI.
    target = tmp_path / "lecture.pdf"
    result = await material_transport(fixtures(pdf_bytes)).commons_pdf(11, 22, target)
    assert target.read_bytes() == pdf_bytes
    assert len(PdfReader(target).pages) == 1
    assert result == {
        "available": True,
        "title": "Synthetic lecture",
        "source": "commons_original",
        "original": True,
        "expected_bytes": len(pdf_bytes),
        "bytes": len(pdf_bytes),
        "content_type": "application/octet-stream",
    }
    assert "synthetic-private" not in json.dumps(result)
    if os.name != "nt":  # Windows chmod does not implement POSIX owner/group modes.
        assert target.stat().st_mode & 0o777 == 0o600


@pytest.mark.parametrize("changes", [{"opened": False}, {"lecture_period_status": "locked"}, {"course_id": 999}, {"item_content_type": "page"}])
async def test_commons_rejects_locked_or_wrong_course_items(material_transport, tmp_path, pdf_bytes, changes):
    # Break caught: module membership or opening restrictions are no longer enforced.
    target = tmp_path / "denied.pdf"
    result = await material_transport(fixtures(pdf_bytes, item_changes=changes)).commons_pdf(11, 22, target)
    assert result["available"] is False
    assert not target.exists()


async def test_commons_rejects_download_redirect_to_untrusted_host(material_transport, tmp_path, pdf_bytes):
    # Break caught: an allowed endpoint can redirect credentials or bytes to another host.
    target = tmp_path / "denied.pdf"
    with pytest.raises(EClassError):
        await material_transport(fixtures(pdf_bytes, redirect="https://evil.test/private.pdf")).commons_pdf(11, 22, target)
    assert not target.exists()


async def test_commons_enforces_stream_limit_and_cleans_partial_file(material_transport, tmp_path, pdf_bytes):
    # Break caught: a response without a Content-Length can exceed its download budget.
    responses = fixtures(pdf_bytes, meta_changes={"original_file_length": 0})
    target = tmp_path / "too-large.pdf"
    with pytest.raises(EClassError):
        await material_transport(responses).commons_pdf(11, 22, target, max_bytes=100)
    assert not target.exists()


async def test_commons_preserves_existing_destination(material_transport, tmp_path, pdf_bytes):
    # Break caught: download collection overwrites an unrelated local file.
    target = tmp_path / "existing.pdf"
    target.write_bytes(b"keep")
    with pytest.raises((EClassError, FileExistsError)):
        await material_transport(fixtures(pdf_bytes)).commons_pdf(11, 22, target)
    assert target.read_bytes() == b"keep"


async def test_commons_has_no_native_pdf_for_deleted_original(material_transport, tmp_path, pdf_bytes):
    # Break caught: a deleted original gets mislabeled as a native download.
    target = tmp_path / "deleted.pdf"
    result = await material_transport(fixtures(pdf_bytes, meta_changes={"is_original_file_deleted": True})).commons_pdf(11, 22, target)
    assert result["available"] is False
    assert not target.exists()


async def test_commons_does_not_call_converted_presentation_an_original_pdf(material_transport, tmp_path, pdf_bytes):
    # Break caught: a rendered presentation is mislabeled as its original PDF.
    target = tmp_path / "converted.pdf"
    result = await material_transport(fixtures(pdf_bytes, meta_changes={"extension": "pptx"})).commons_pdf(11, 22, target)
    assert result["available"] is False
    assert "native original PDF" in result["reason"]
    assert not target.exists()


async def test_commons_rejects_untrusted_xml_download_uri(material_transport, tmp_path, pdf_bytes):
    # Break caught: URL validation is applied only to the first Commons endpoint.
    responses = fixtures(pdf_bytes)
    responses["https://ocs.cau.ac.kr/content/fixture.xml?key=synthetic-private"]["text"] = "<content_download_uri>https://evil.test/original.pdf</content_download_uri>"
    responses["https://evil.test/original.pdf"] = {"base64": base64.b64encode(pdf_bytes).decode(), "type": "application/pdf"}
    target = tmp_path / "foreign.pdf"
    with pytest.raises(EClassError):
        await material_transport(responses).commons_pdf(11, 22, target)
    assert not target.exists()


async def test_commons_checks_declared_original_size(material_transport, tmp_path, pdf_bytes):
    # Break caught: truncated native PDF bytes survive transfer validation.
    target = tmp_path / "truncated.pdf"
    responses = fixtures(pdf_bytes, meta_changes={"original_file_length": len(pdf_bytes) + 1})
    with pytest.raises(EClassError):
        await material_transport(responses).commons_pdf(11, 22, target)
    assert not target.exists()


async def test_aside_collect_cleans_partial_copy(tmp_path, monkeypatch):
    # Break caught: a local collection failure leaves an incomplete output document.
    source = tmp_path / ".aside" / "part"
    source.parent.mkdir()
    source.write_bytes(b"document")
    target = tmp_path / "partial.pdf"
    monkeypatch.setattr(Path, "home", staticmethod(lambda: tmp_path))

    def fail_copy(input_file, output_file):
        output_file.write(b"partial")
        raise OSError("synthetic disk failure")

    monkeypatch.setattr(shutil, "copyfileobj", fail_copy)
    with pytest.raises(OSError):
        AsideTransport().collect_file({"parts": [str(source)]}, target)
    assert not target.exists()
    assert not source.exists()
