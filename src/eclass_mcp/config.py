"""Transport selection shared by the CLI and MCP lifespan."""

import os
import shutil

from .cache import CachingTransport
from .session import default_session_file
from .transport import AsideTransport, EClassError

BACKENDS = ("http", "aside", "cdp")


def resolve_backend() -> str:
    """Prefer the saved HTTP session, then an already installed Aside CLI."""
    configured = os.getenv("ECLASS_BROWSER", "").strip().casefold()
    if configured:
        return configured
    if default_session_file().exists():
        return "http"
    if shutil.which(os.getenv("ECLASS_ASIDE_BIN", "aside")):
        return "aside"
    return "http"


def create_transport():
    backend = resolve_backend()
    if backend == "http":
        from .http_transport import HTTPTransport

        inner = HTTPTransport.from_env()
    elif backend == "aside":
        inner = AsideTransport()
    elif backend == "cdp":
        from .cdp import CDPTransport

        inner = CDPTransport()
    else:
        raise EClassError("ECLASS_BROWSER must be http, aside or cdp")
    try:
        ttl = float(os.getenv("ECLASS_CACHE_TTL", "20"))
    except ValueError:
        ttl = 20.0
    return CachingTransport(inner, ttl) if ttl > 0 else inner
