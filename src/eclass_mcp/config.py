"""Transport selection shared by the CLI and MCP lifespan."""

import os

from .transport import AsideTransport, EClassError


def create_transport():
    backend = os.getenv("ECLASS_BROWSER", "aside")
    if backend == "aside":
        return AsideTransport()
    if backend == "cdp":
        from .cdp import CDPTransport

        return CDPTransport()
    raise EClassError("ECLASS_BROWSER must be aside or cdp")
