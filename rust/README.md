# Rust MCP

This directory is the primary Rust implementation of the CAU e-Class MCP. The
Python package is now only the optional OZ PDF helper.

## Status

- **Ported:** session loading, Chrome profile cookie import, HTTP GET/pagination,
  TTL cache with one loader per key, course/todo/briefing, weekly materials,
  Commons PDF and video downloads, Canvas files, assignments/deadlines,
  announcements, syllabus text, boards, groups, PDF validation/page text,
  Canvas upload/submission over HTTP, and 28 MCP tool schemas.
- **Optional Python helper:** OZ native PDF export (`download_syllabus_pdf`).
  Rust invokes `eclass-oz-export` (Playwright) only for this tool; it returns a
  clear error when the helper is not installed.
- **Removed:** the legacy `aside`/CDP transport switch and the FastMCP Python
  server. Rust always uses the saved HTTP session. `login` creates the Canvas
  session; the LearningX token is minted from the LTI launch without a browser.

## Build and run

```sh
cd rust
cargo build --release
./target/release/eclass-mcp-rs --help
```

The Rust binary creates the same `session.json` used by the OZ helper. Create it
once with Rust:

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

The session file is shared with the optional Python OZ helper: macOS
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

The intended user path is **GitHub Releases**, not a local Rust install:

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.sh | sh
eclass-mcp onboard --client claude
```

Users need no Rust, Python, uv, or virtualenv for the 26 HTTP tools. The release
workflow builds macOS arm64/x86_64, Linux x86_64/arm64, and Windows x64 archives
with SHA-256 files. The first beta is unsigned; the release notes must say so,
and Windows may show SmartScreen. OZ PDF is optional and needs the small
`eclass-oz-export` Python helper. See [RELEASING.md](RELEASING.md) for version
pinning, the update path, signing, notarization, and the optional Homebrew tap.


## Verification

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

The test suite includes a synthetic wiremock Canvas/LearningX server, a real
PDF fixture, session/cookie rules, cache behavior, a fake OZ helper invocation,
and MCP initialize/tools calls. It does not use a real school account.

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
reads/downloads; the Python browser mode required an external Chrome tree (about
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

## Can Python be removed?

For the core read/download path, **yes**. Nothing there needs Python at
runtime:

- session/cookie handling, Chrome profile cookie import, HTTP, pagination, cache, catalog,
  briefing, PDF validation/text and MCP stdio all have Rust implementations.
- `pypdf` is replaced by `lopdf`; the fixture and integration tests cover the
  normal page-text path.
- FastMCP, Aside, and CDP are removed; the OZ helper is the only optional Python process.

The only remaining browser-bound feature is:

1. OZ native PDF export (`download_syllabus_pdf`): DOM clicks, a Save dialog and
   a browser download event.

It is **not a Python-language requirement**; it is a browser UI flow. The Rust
core invokes an optional `eclass-oz-export` helper with Playwright. Assignment
submission is already Rust HTTP, synthetic-tested and disabled by default;
real-account verification remains separate.

