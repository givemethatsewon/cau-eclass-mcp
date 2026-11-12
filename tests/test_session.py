from __future__ import annotations

import json
import os
import time

import pytest

from eclass_mcp.errors import EClassError
from eclass_mcp.session import Session, load_session, save_session


def test_session_filters_cau_cookies_and_reads_token(tmp_path):
    path = tmp_path / "session.json"
    path.write_text(
        json.dumps(
            {
                "version": 1,
                "cookies": [
                    {"name": "sid", "value": "ok", "domain": ".cau.ac.kr", "path": "/"},
                    {"name": "other", "value": "no", "domain": ".example.com", "path": "/"},
                    {
                        "name": "old",
                        "value": "no",
                        "domain": ".cau.ac.kr",
                        "path": "/",
                        "expires": time.time() - 10,
                    },
                ],
            }
        ),
        encoding="utf-8",
    )
    session = load_session(path)
    assert session is not None
    assert [cookie["name"] for cookie in session.cookies] == ["sid", "old"]
    assert session.token("sid") == "ok"
    assert session.token("old") is None


def test_session_rejects_unknown_version():
    with pytest.raises(EClassError):
        Session.from_json({"version": 2, "cookies": []})
    with pytest.raises(EClassError):
        Session.from_json({"version": 1, "cookies": [{"name": "sid", "value": None}]})


def test_save_session_is_owner_only(tmp_path):
    path = save_session(Session(cookies=[{"name": "sid", "value": "x"}]), tmp_path / "s.json")
    assert json.loads(path.read_text())["version"] == 1
    if os.name != "nt":
        assert path.stat().st_mode & 0o777 == 0o600
