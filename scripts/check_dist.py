"""Inspect distribution contents without credentials or browser access."""

import tarfile
import zipfile
from pathlib import Path

FORBIDDEN = {"analysis", "downloads", "test-results", "private", ".session", ".credentials", ".secrets", ".venv"}


def check(names):
    for name in names:
        parts = Path(name).parts
        if FORBIDDEN.intersection(parts) or Path(name).name in {"mcp.json", ".env", "storage-state.json"}:
            raise SystemExit(f"Private artifact included in distribution: {name}")
        if Path(name).name in {"verification.md", "probe_aside.py"} or Path(name).name.startswith("live_"):
            raise SystemExit(f"Private verification artifact included in distribution: {name}")
        if any(part.endswith((".har", ".cookies", ".pem", ".key")) for part in parts):
            raise SystemExit(f"Credential/debug artifact included in distribution: {name}")


if __name__ == "__main__":
    wheels = list(Path("dist").glob("cau_eclass_mcp-*.whl"))
    sources = list(Path("dist").glob("cau_eclass_mcp-*.tar.gz"))
    assert wheels and sources, "Build both wheel and sdist first"
    for wheel in wheels:
        with zipfile.ZipFile(wheel) as archive:
            names = archive.namelist()
            check(names)
            assert "eclass_mcp/SKILL.md" in names
            assert any(name.endswith("/licenses/LICENSE") for name in names)
            assert any(name.endswith("/licenses/NOTICE") for name in names)
            metadata_name = next(name for name in names if name.endswith(".dist-info/METADATA"))
            assert "License-Expression: LicenseRef-CAU-EClass-Noncommercial-1.0" in archive.read(metadata_name).decode()
            assert "eclass_mcp/cli.py" in names
            assert "eclass_mcp/cdp.py" in names
    for source in sources:
        with tarfile.open(source) as archive:
            check(archive.getnames())
    print(f"Checked {len(wheels)} wheel(s) and {len(sources)} sdist(s): no private artifacts")
