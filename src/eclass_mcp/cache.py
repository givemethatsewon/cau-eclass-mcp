"""Small TTL cache with request coalescing for repeated read-only calls."""

from __future__ import annotations

import asyncio
import copy
import os
import time


class CachingTransport:
    """Cache GET responses and share one in-flight request per key.

    Tool results are often read more than once (a briefing, a deadline view and a
    material list may all touch the same module response).  A short TTL keeps the
    data fresh without turning every tool call into a new school request.
    """

    def __init__(self, inner, ttl: float | None = None):
        self.inner = inner
        self.ttl = float(os.getenv("ECLASS_CACHE_TTL", "20")) if ttl is None else float(ttl)
        self._values: dict[tuple, tuple[float, dict]] = {}
        self._inflight: dict[tuple, asyncio.Task] = {}
        self._generation = 0
        if hasattr(inner, "on_refresh"):
            inner.on_refresh = self.clear

    async def __aenter__(self):
        await self.inner.__aenter__()
        return self

    async def __aexit__(self, *args):
        return await self.inner.__aexit__(*args)

    def __getattr__(self, name):
        return getattr(self.inner, name)

    def clear(self):
        self._generation += 1
        self._values.clear()

    async def get(self, path: str, learningx: bool = False, html: bool = False):
        if self.ttl <= 0:
            return await self.inner.get(path, learningx, html)
        key = (path, learningx, html)
        now = time.monotonic()
        entry = self._values.get(key)
        if entry is not None and entry[0] > now:
            return copy.deepcopy(entry[1])

        task = self._inflight.get(key)
        if task is None:
            task = asyncio.create_task(self._load(key))
            self._inflight[key] = task

            def finished(done, key=key):
                if self._inflight.get(key) is done:
                    self._inflight.pop(key, None)

            task.add_done_callback(finished)
            value = await task
        else:
            value = await asyncio.shield(task)
        return copy.deepcopy(value)

    async def _load(self, key: tuple) -> dict:
        path, learningx, html = key
        generation = self._generation
        value = await self.inner.get(path, learningx, html)
        if (
            generation == self._generation
            and isinstance(value, dict)
            and int(value.get("status", 0)) < 400
        ):
            self._values[key] = (time.monotonic() + self.ttl, copy.deepcopy(value))
        return value
