import json
import tomllib
from pathlib import Path

import httpx
import pytest

from eclass_mcp.cdp import CDPTransport, trusted_download, validate_endpoint
from eclass_mcp.cli import main
from eclass_mcp.config import create_transport
from eclass_mcp.transport import EClassError


@pytest.mark.parametrize(
    "endpoint",
    [
        "https://example.com:9222",
        "http://127.0.0.1:9222?token=private",
        "http://user:pass@localhost:9222",
        "file:///tmp/devtools",
        "http://0.0.0.0:9222",
    ],
)
def test_cdp_rejects_remote_or_secret_endpoint(endpoint):
    with pytest.raises(EClassError):
        validate_endpoint(endpoint)


def test_cdp_loopback_and_config(monkeypatch, capsys):
    for endpoint in ("http://127.0.0.1:9222", "ws://[::1]:9222/devtools/browser/test"):
        assert validate_endpoint(endpoint) == endpoint
    monkeypatch.setenv("ECLASS_BROWSER", "aside")
    monkeypatch.delenv("ECLASS_CDP_ENDPOINT", raising=False)
    monkeypatch.setenv("ECLASS_ENABLE_SUBMISSION", "1")
    assert main(["--cdp-endpoint", "http://127.0.0.1:9222", "config", "--client", "codex"]) == 0
    config = tomllib.loads(capsys.readouterr().out)["mcp_servers"]["eclass"]
    assert Path(config["command"]).is_file()
    assert config["env"]["ECLASS_BROWSER"] == "cdp"
    assert config["env"]["ECLASS_ENABLE_SUBMISSION"] == "0"
    assert isinstance(create_transport(), CDPTransport)
    main(["config", "--client", "vscode"])
    assert json.loads(capsys.readouterr().out)["servers"]["eclass"]["type"] == "stdio"


def test_unknown_transport_fails(monkeypatch):
    monkeypatch.setenv("ECLASS_BROWSER", "typo")
    with pytest.raises(EClassError, match="aside or cdp"):
        create_transport()


@pytest.mark.parametrize(
    "url",
    [
        "https://evil.test/file",
        "http://ocs.cau.ac.kr/file",
        "https://ocs.cau.ac.kr:1234/file",
        "https://x:secret@ocs.cau.ac.kr/file",
    ],
)
def test_reject_download_host(url):
    with pytest.raises(EClassError):
        trusted_download(url)


async def test_stream_size_limit_and_no_overwrite(tmp_path):
    target = tmp_path / "download"
    response = httpx.Response(200, content=b"123456", headers={"content-type": "video/mp4"})
    with pytest.raises(EClassError, match="limit"):
        await CDPTransport.save_response(response, target, 4)
    assert not target.exists()
    target.write_bytes(b"keep")
    with pytest.raises(FileExistsError):
        await CDPTransport.save_response(response, target, 100)
    assert target.read_bytes() == b"keep"
    target.unlink()
    with pytest.raises(EClassError, match="mismatch"):
        await CDPTransport.save_response(response, target, 100, expected=7)
    assert not target.exists()


async def test_redirect_cookie_scope_and_host_guard(monkeypatch):
    calls = []
    real_client = httpx.AsyncClient

    def handle(request):
        calls.append(request)
        if request.url.host == "eclass3.cau.ac.kr":
            return httpx.Response(302, headers={"location": "https://ocs.cau.ac.kr/file"})
        return httpx.Response(302, headers={"location": "https://evil.test/file"})

    monkeypatch.setattr(
        httpx, "AsyncClient", lambda **kw: real_client(transport=httpx.MockTransport(handle), **kw)
    )

    class Context:
        async def cookies(self, urls):
            return [{"name": "session", "value": "synthetic"}] if "eclass3" in urls[0] else []

    with pytest.raises(EClassError, match="Untrusted"):
        async with CDPTransport().response(Context(), "https://eclass3.cau.ac.kr/file"):
            pytest.fail("must reject redirect")
    assert len(calls) == 2
    assert calls[0].headers["cookie"] == "session=synthetic"
    assert "cookie" not in calls[1].headers and "authorization" not in calls[1].headers
