"""Real CDP protocol, synthetic site responses. No university login or network writes."""

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest
import pytest_asyncio

from eclass_mcp.cdp import CDPTransport
from eclass_mcp.service import EClassService
from eclass_mcp.transport import ORIGIN, EClassError


@pytest_asyncio.fixture
async def chromium_endpoint(tmp_path):
    executable = os.getenv("ECLASS_TEST_CHROME")
    if not executable:
        pytest.skip("Set ECLASS_TEST_CHROME to run real Chromium CDP tests")
    profile = tmp_path / "chromium"
    args = [
        executable,
        "--headless=new",
        "--remote-debugging-address=127.0.0.1",
        "--remote-debugging-port=0",
        f"--user-data-dir={profile}",
        "--no-first-run",
        "--no-default-browser-check",
        "about:blank",
    ]
    # Hosted Linux runners restrict Chromium's user namespaces. This applies only
    # to the disposable synthetic-test browser, never the user's CDP browser.
    if sys.platform.startswith("linux") and (
        os.getuid() == 0 or os.getenv("GITHUB_ACTIONS") == "true"
    ):
        args.append("--no-sandbox")
    process = await asyncio.create_subprocess_exec(
        *args, stdout=asyncio.subprocess.DEVNULL
    )
    try:
        async with asyncio.timeout(30):
            while not (profile / "DevToolsActivePort").exists():
                if process.returncode is not None:
                    pytest.fail("Test Chromium exited during startup")
                await asyncio.sleep(0.1)
        port = (profile / "DevToolsActivePort").read_text().splitlines()[0]
        yield f"http://127.0.0.1:{port}"
    finally:
        if process.returncode is None:
            process.terminate()
            try:
                await asyncio.wait_for(process.wait(), timeout=10)
            except TimeoutError:
                process.kill()
                await process.wait()


async def test_real_cdp_read_pdf_and_disconnect(chromium_endpoint, tmp_path):
    import base64
    import io

    from playwright.async_api import async_playwright
    from pypdf import PdfWriter

    pdf = io.BytesIO()
    writer = PdfWriter()
    writer.add_blank_page(width=100, height=100)
    writer.write(pdf)
    async with CDPTransport(chromium_endpoint) as transport:
        context = transport.browser.contexts[0]
        await context.add_cookies([{"name": "xn_api_token", "value": "synthetic", "url": ORIGIN}])
        requests = []

        async def route(request):
            url = request.request.url
            requests.append((request.request.method, url))
            if "/api/v1/users/self/profile" in url:
                await request.fulfill(json={"id": 1, "name": "Fixture", "access_token": "synthetic-secret"})
            elif "/learningx/api/v1/courses/1/modules" in url:
                assert request.request.headers.get("authorization") == "Bearer synthetic"
                await request.fulfill(
                    json=[
                        {
                            "module_items": [
                                {
                                    "content_data": {
                                        "item_id": 2,
                                        "opened": False,
                                        "item_content_data": {
                                            "content_id": "not_open",
                                            "content_type": "mp4",
                                        },
                                    }
                                }
                            ]
                        }
                    ]
                )
            elif "/assignments/syllabus" in url:
                await request.fulfill(
                    content_type="text/html",
                    body='<iframe src="https://rpt80.cau.ac.kr/oz80/ozViewer.jsp"></iframe>',
                )
            elif "rpt80.cau.ac.kr" in url:
                await request.fulfill(
                    content_type="text/html",
                    body=f"""<input type="button" class="btnSAVEAS" title="Save" onclick="document.querySelector('[role=dialog]').hidden=false">
<div role="dialog" aria-label="Save" hidden><select class="oz_ui_layout"><option>Adobe PDF File(*.pdf)</option></select>
<button onclick="const a=document.createElement('a');a.href=window.URL.createObjectURL(new Blob([Uint8Array.from(atob('{base64.b64encode(pdf.getvalue()).decode()}'),c=>c.charCodeAt(0))],{{type:'application/pdf'}}));a.download='syllabus.pdf';a.click()">OK</button></div>""",
                ) if not url.endswith("fixture.pdf") else await request.fulfill(
                    content_type="application/pdf",
                    body=pdf.getvalue(),
                    headers={"content-disposition": "attachment; filename=syllabus.pdf"},
                )
            else:
                await request.fulfill(content_type="text/html", body="<h1>Fixture</h1>")

        await context.route("**/*", route)
        page = await context.new_page()
        await page.goto(ORIGIN + "/courses/1")
        result = await transport.get("/api/v1/users/self/profile")
        assert result["body"]["id"] == 1
        assert "synthetic-secret" not in json.dumps(result)
        assert (await transport.get("/learningx/api/v1/courses/1/modules", learningx=True))["status"] == 200
        with pytest.raises(EClassError, match="not open"):
            await EClassService(transport, output_dir=tmp_path).download_video(1, 2)
        count = len(context.pages)
        result = await EClassService(transport, output_dir=tmp_path).syllabus_document(1)
        assert result["page_count"] == 1
        assert result["text_complete"] is False
        assert Path(result["path"]).read_bytes().startswith(b"%PDF-")
        assert len(context.pages) == count
        assert not page.is_closed()
        from fastmcp import Client
        from fastmcp.client.transports import StdioTransport

        async with Client(
            StdioTransport(
                command=sys.executable,
                args=[
                    "-m",
                    "eclass_mcp.cli",
                    "--browser",
                    "cdp",
                    "--cdp-endpoint",
                    chromium_endpoint,
                    "serve",
                ],
                env={"ECLASS_ENABLE_SUBMISSION": "0"},
            )
        ) as client:
            assert len(await client.list_tools()) == 21
            result = (await client.call_tool("get_session_status", {})).data["result"]
            assert result["authenticated"] is True
            assert result["submission_enabled"] is False
        assert all(method == "GET" for method, _ in requests)

    async with async_playwright() as control:
        browser = await control.chromium.connect_over_cdp(chromium_endpoint)
        assert any(p.url == ORIGIN + "/courses/1" for p in browser.contexts[0].pages)
