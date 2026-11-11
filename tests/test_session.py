"""Saved-session format and privacy boundaries."""

from __future__ import annotations

import json
import os
import stat
import time

import httpx
import pytest

from eclass_mcp.session import (
    Session,
    SessionExpired,
    default_session_file,
    delete_session,
    load_session,
    merge_cookies,
    save_session,
)
from eclass_mcp.transport import EClassError


def test_session_file_is_owner_only_and_round_trips(tmp_path, monkeypatch):
    path = tmp_path / "nested" / "session.json"
    monkeypatch.setenv("ECLASS_SESSION_FILE", str(path))
    assert default_session_file() == path
    session = Session(
        cookies=[{"name": "sid", "value": "synthetic", "domain": ".cau.ac.kr", "path": "/"}],
        endpoint="http://127.0.0.1:9222",
    )
    assert save_session(session) == path
    if os.name != "nt":
        assert stat.S_IMODE(path.stat().st_mode) == 0o600
        assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    loaded = load_session()
    assert loaded is not None
    assert loaded.token("sid") == "synthetic"
    assert loaded.endpoint == "http://127.0.0.1:9222"
    assert "synthetic" in path.read_text(encoding="utf-8")
    assert delete_session(path) is True
    assert delete_session(path) is False
    assert load_session(path) is None


def test_cdp_cookies_are_filtered_and_deduplicated():
    session = Session.from_cdp(
        [
            {"name": "sid", "value": "a", "domain": ".cau.ac.kr", "path": "/"},
            {"name": "sid", "value": "b", "domain": ".cau.ac.kr", "path": "/"},
            {"name": "other", "value": "c", "domain": ".example.com", "path": "/"},
            {"name": "canvas", "value": "d", "domain": "canvas.cau.ac.kr", "path": "/"},
        ]
    )
    assert session.token("sid") == "b"
    assert session.token("canvas") == "d"
    assert session.token("other") is None


def test_expired_cookies_are_not_sent():
    session = Session(
        cookies=[
            {"name": "old", "value": "x", "domain": ".cau.ac.kr", "path": "/", "expires": time.time() - 10},
            {"name": "new", "value": "y", "domain": ".cau.ac.kr", "path": "/", "expires": time.time() + 600},
        ]
    )
    names = {cookie.name for cookie in session.as_cookies().jar}
    assert names == {"new"}
    assert session.token("old") is None
    assert session.token("new") == "y"


def test_invalid_session_file_has_actionable_error(tmp_path):
    path = tmp_path / "session.json"
    path.write_text(json.dumps({"version": 99, "cookies": []}), encoding="utf-8")
    with pytest.raises(EClassError, match="Unsupported"):
        load_session(path)
    path.write_text("not json", encoding="utf-8")
    with pytest.raises(EClassError, match="login"):
        load_session(path)


def test_merge_cookies_keeps_only_cau_domains():
    session = Session()
    jar = httpx.Cookies()
    jar.set("sid", "value", domain=".cau.ac.kr", path="/")
    jar.set("foreign", "value", domain=".example.com", path="/")
    merge_cookies(session, jar)
    assert session.token("sid") == "value"
    assert session.token("foreign") is None


def test_session_expired_is_transport_error():
    assert issubclass(SessionExpired, EClassError)
