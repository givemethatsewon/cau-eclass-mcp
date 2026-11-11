"""TTL cache and request coalescing."""

from __future__ import annotations

import asyncio

import pytest

from eclass_mcp.cache import CachingTransport
from eclass_mcp.transport import EClassError


class Inner:
    kind = "fake"
    on_refresh = None

    def __init__(self):
        self.calls = 0
        self.value = {"body": {"items": [1]}, "status": 200}
        self.started = asyncio.Event()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return None

    async def get(self, path, learningx=False, html=False):
        self.calls += 1
        self.started.set()
        await asyncio.sleep(0.01)
        return self.value

    async def download(self, path, destination, max_bytes):
        return {"bytes": 1}


async def test_cache_hits_return_independent_copies():
    inner = Inner()
    cached = CachingTransport(inner, ttl=60)
    first = await cached.get("/x")
    first["body"]["items"].append(2)
    second = await cached.get("/x")
    assert second["body"]["items"] == [1]
    assert inner.calls == 1
    assert cached.kind == "fake"


async def test_single_flight_shares_one_request():
    inner = Inner()
    cached = CachingTransport(inner, ttl=60)
    first, second = await asyncio.gather(cached.get("/x"), cached.get("/x"))
    assert first == second
    assert inner.calls == 1


async def test_ttl_expiry_and_error_not_cached():
    inner = Inner()
    cached = CachingTransport(inner, ttl=0.01)
    await cached.get("/x")
    await asyncio.sleep(0.02)
    await cached.get("/x")
    assert inner.calls == 2

    class Failing(Inner):
        async def get(self, *args, **kwargs):
            self.calls += 1
            raise EClassError("synthetic")

    failing = Failing()
    cached = CachingTransport(failing, ttl=60)
    with pytest.raises(EClassError):
        await cached.get("/x")
    with pytest.raises(EClassError):
        await cached.get("/x")
    assert failing.calls == 2


async def test_zero_ttl_bypasses_cache():
    inner = Inner()
    cached = CachingTransport(inner, ttl=0)
    await cached.get("/x")
    await cached.get("/x")
    assert inner.calls == 2


async def test_refresh_clears_cached_data():
    inner = Inner()
    cached = CachingTransport(inner, ttl=60)
    await cached.get("/x")
    assert inner.calls == 1
    inner.on_refresh()
    await cached.get("/x")
    assert inner.calls == 2
