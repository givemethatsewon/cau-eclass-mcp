"""Install the wheel in a temporary uv tool environment and verify the helper CLI."""

import os
import subprocess
import tempfile
from pathlib import Path


def run(args, *, env, cwd):
    return subprocess.run(
        args, env=env, cwd=cwd, check=True, encoding="utf-8", capture_output=True
    ).stdout


def main():
    wheel = sorted(Path("dist").glob("cau_eclass_mcp-*.whl"))[-1].resolve()
    with tempfile.TemporaryDirectory(prefix="eclass-install-") as directory:
        root = Path(directory)
        env = {**os.environ, "UV_TOOL_DIR": str(root / "tools"), "UV_TOOL_BIN_DIR": str(root / "bin")}
        run(
            [
                "uv",
                "tool",
                "install",
                "--python",
                "3.12",
                "--from",
                str(wheel),
                "cau-eclass-mcp",
            ],
            env=env,
            cwd=root,
        )
        executable = root / "bin" / ("eclass-oz-export.exe" if os.name == "nt" else "eclass-oz-export")
        help_text = run([str(executable), "--help"], env=env, cwd=root)
        assert "OZ syllabus PDF" in help_text
        print("Clean wheel installation OK: eclass-oz-export starts and exposes --help")


if __name__ == "__main__":
    main()
