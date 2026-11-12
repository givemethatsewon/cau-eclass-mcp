"""Errors shared by the OZ export helper."""

from __future__ import annotations

ORIGIN = "https://eclass3.cau.ac.kr"


class EClassError(Exception):
    """A safe, user-facing helper error."""
