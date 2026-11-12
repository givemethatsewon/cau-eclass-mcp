# Rust porting notes

Source of truth: `rust/src` (`eclass_core` library + `eclass-mcp-rs` binary).
The Python package is now only an optional OZ PDF helper.

## API mapping

| Python | Rust |
|---|---|
| `session.Session` / `Cookie` | `session::Session` / `session::Cookie` |
| `session.read_browser_cookies` | `session::read_browser_cookies` |
| `session.login_from_profile` | `session::login_from_profile` |
| `session.refresh_session` | `session::refresh_session` |
| `session.save_session` / `load_session` | same names in `session` |
| `http.HTTPTransport` | `http::HttpTransport` |
| `cache.CachingTransport` | `HttpTransport`'s `TtlCache` |
| `catalog.Material` | `catalog::Material` |
| `catalog.material_catalog` / `filter_materials` | same names in `catalog` |
| `documents.inspect_pdf` / `read_pdf` | same names in `documents` |
| `service.EClassService` | `service::EClassService` |
| FastMCP tool registration | `mcp::tools()` + `mcp::serve_stdio` |
| `download_syllabus_pdf` | `syllabus_document` invokes `eclass-oz-export` (optional Python/Playwright) |
| `prepare_submission` / `submit_assignment` | Canvas HTTP upload/submission + readback verification |

## Test mapping

| Python test area | Rust test |
|---|---|
| `tests/test_session.py` | `src/session.rs` unit tests |
| legacy Python HTTP/cache/catalog tests | removed; `src/http.rs` + `src/catalog.rs` tests |
| `tests/service.rs` briefing/material tests | `tests/service.rs` |
| legacy CDP browser tests | removed with the CDP adapter |
| PDF page text | `tests/pdf.rs` with `tests/fixtures/sample.pdf` |

## Deliberate differences

- **OZ PDF is the only browser helper.** Rust calls `eclass-oz-export` only for
  that tool. Assignment preparation and submission are implemented in Rust
  using Canvas HTTP upload tickets, CSRF, multipart upload and readback; they
  still need real-account validation before routine use.
- **PDF text uses `lopdf`.** Text extraction may differ from Python `pypdf` on
  unusual fonts or layouts. The fixture covers the normal text path.
- **`list_learning_items(detail=true)` returns the raw provider modules
  without applying week/query/type filters.** The compact/default path is fully
  ported; this legacy detail path is intentionally left unfiltered in the
  experimental Rust core.
- **Only HTTP transport is available.** FastMCP, Aside, and CDP were removed.
  Rust reads Chrome profile cookies for the one-time `login` and session
  refresh; no remote debugging port is opened.
- **MCP protocol version is `2024-11-05`.** The Rust server exposes 27 tools.
  The legacy FastMCP client accepted the server before FastMCP was removed;
  current clients use raw stdio JSON-RPC.
- **Single-account process.** Like the Python service, caches and the saved
  session assume one account per process.

## Preserved quirks

- `observed_at` + `result` envelope, `complete` / `next_cursor` fields, and
  Asia/Seoul `_seoul` date fields.
- `resource_id` remains `commons:<course>:<item>` / `canvas:<course>:<file>`.
- PDF text offsets are character offsets, not byte offsets.
- Downloads never overwrite an existing path; failed downloads remove the
  partial file.
- HTTP redirects are followed only within the configured origin; download
  hosts are checked per redirect.
- Sensitive URL query values and secret-looking response keys are redacted.

## Verification

Run from `rust/`:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

Results at the time of writing: 20 unit tests, 2 PDF integration tests, 8
service integration tests, clippy clean, release build successful.
