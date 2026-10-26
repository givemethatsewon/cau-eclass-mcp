"""Install the wheel in a temporary uv tool environment and verify the public CLI."""

import json
import os
import subprocess
import tempfile
from pathlib import Path


def run(args, *, env, cwd):
    return subprocess.run(args, env=env, cwd=cwd, check=True, text=True, capture_output=True).stdout


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
                "--with",
                "playwright>=1.60,<2",
                "cau-eclass-mcp",
            ],
            env=env,
            cwd=root,
        )
        executable = root / "bin" / ("eclass-mcp.exe" if os.name == "nt" else "eclass-mcp")
        version = run([str(executable), "--version"], env=env, cwd=root).strip()
        skill = run([str(executable), "skill"], env=env, cwd=root)
        assert "name: cau-eclass" in skill
        config = json.loads(run([str(executable), "--browser", "cdp", "config"], env=env, cwd=root))
        entry = config["mcpServers"]["eclass"]
        assert entry["env"]["ECLASS_BROWSER"] == "cdp"
        # Verify config preserves the virtualenv interpreter, rather than resolving its symlink.
        assert (
            run([entry["command"], "-m", "eclass_mcp.cli", "--version"], env=env, cwd=root).strip() == version
        )
        result = subprocess.run(
            [str(executable), "--cdp-endpoint", "http://127.0.0.1:1", "doctor"],
            env=env,
            cwd=root,
            text=True,
            capture_output=True,
        )
        assert result.returncode == 1
        assert json.loads(result.stdout)["ok"] is False
        print(
            f"Clean wheel installation OK: {version}, bundled skill, generated config, doctor failure contract"
        )


if __name__ == "__main__":
    main()
