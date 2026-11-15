# Rust MCP

The CAU e-Class MCP ships as one Rust executable. It handles course and material
reads, Canvas downloads and submissions, PDF text extraction, and native OZ
syllabus export.

`download_syllabus_pdf` opens an installed Chrome, Edge, Brave, or Chromium
browser in a temporary profile. It drives the OZ viewer's Save → Adobe PDF
flow and returns the verified PDF and optional page text. Set
`ECLASS_OZ_BROWSER` if the executable is outside the standard locations.

## Build and run

```sh
cd rust
cargo build --release
./target/release/eclass-mcp-rs --help
```

Create the e-Class session once:

```sh
./target/release/eclass-mcp-rs login
```

Then:

```sh
./target/release/eclass-mcp-rs onboard --client claude
./target/release/eclass-mcp-rs doctor
./target/release/eclass-mcp-rs config --client claude
./target/release/eclass-mcp-rs serve
```

`login`, `auth refresh`, `onboard`, `logout`, `doctor`, `config`, and `serve`
are available directly in Rust too. `onboard` opens a dedicated Chromium profile, waits for
the user to sign in, verifies the profile and course list, mints the
LearningX token, and prints structured next actions. The MCP tool
`get_onboarding_status` exposes the same state to an agent. The schema and
action codes are documented in [ONBOARDING.md](ONBOARDING.md).

The session file is stored in macOS
`~/Library/Application Support/cau-eclass/session.json`, Linux
`${XDG_CONFIG_HOME:-~/.config}/cau-eclass/session.json`, Windows
`%APPDATA%\cau-eclass\session.json`. It contains only `.cau.ac.kr` cookies and
is written `0600`; `eclass-mcp logout` deletes it. It is a bearer credential,
so never sync or share it.

The MCP server can start with **no saved session**. In that state
`get_onboarding_status` returns `session_state=missing` and the other tools
return a login-required error, so an agent can guide the first run from inside
the MCP client instead of failing at startup.

## Distribution

Install the prebuilt binary from [GitHub Releases](https://github.com/givemethatsewon/cau-eclass-mcp/releases/latest).

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.sh | bash
```

On Windows x64, run this in PowerShell:

```powershell
irm https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.ps1 | iex
```

Run the `onboard` command printed by the installer. Releases include macOS
Apple Silicon, Linux x86_64/ARM64, and Windows x64 archives with SHA-256 files.
See [RELEASING.md](RELEASING.md) for version pinning and update instructions.

## Verification

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

The test suite includes a synthetic wiremock Canvas/LearningX server, a real
PDF fixture, session/cookie rules, cache behavior, and MCP initialize/tools calls.
The Chromium integration test uses a local cross-origin OZ fixture:

```sh
cargo test --locked oz::tests::exports_native_pdf_from_cross_origin_oz_frame -- --ignored
```

## Measured locally

Same local harness with a fake saved session and no e-Class network calls,
Apple M3 Max. `tools/list` includes process startup, MCP
initialize, and one tool listing; CPU is user+system from `/usr/bin/time -l`.

| Server | Wall time | CPU time | Max RSS | Idle RSS | Processes |
|---|---:|---:|---:|---:|---:|
| Python 0.4.0 | ~0.58–0.63 s | ~0.62 s | ~106 MiB | ~106–111 MiB | 1 |
| Rust core | **~0.04 s** | **<0.01 s** | **~10.5 MiB** | **~9.6 MiB** | 1 |

Approximate improvement: **18–21× faster startup**, **~90–91% less memory**,
and a **17 MiB single release binary**. The Rust server spawns no browser for
the reads/downloads measured here; the Python browser mode required an external Chrome tree (about
674 MiB / 7 processes in the local test).

These are harness measurements, not real e-Class latency. Network time
dominates actual tool calls; the startup/memory gains are independent of the
school API. Actual PDF text extraction can differ from `pypdf` on unusual
fonts/layouts.

## Design notes

- `session.rs` owns the compatible session format and Chrome profile cookie import.
- `http.rs` owns redirects, cookie scoping, downloads and the TTL cache.
- `catalog.rs` is a pure port of the material/week filters.
- `documents.rs` uses `lopdf` for page-by-page text; extraction can differ
  slightly from Python `pypdf`.
- `service.rs` owns pagination, briefing concurrency, deadlines and downloads.
- `mcp.rs` is a small stdio JSON-RPC server rather than a framework, keeping
  startup and memory low.
