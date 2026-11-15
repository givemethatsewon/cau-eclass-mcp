# Rust porting notes

Source of truth: `rust/src` (`eclass_core` library + `eclass-mcp-rs` binary).
The Rust executable includes native OZ PDF export.

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
| `download_syllabus_pdf` | `syllabus_document` drives the installed Chromium browser and validates the result |
| `prepare_submission` / `submit_assignment` | Canvas HTTP upload/submission + readback verification |

## Test mapping

| Python test area | Rust test |
|---|---|
| `tests/test_session.py` | `src/session.rs` unit tests |
| legacy Python HTTP/cache/catalog tests | removed; `src/http.rs` + `src/catalog.rs` tests |
| `tests/service.rs` briefing/material tests | `tests/service.rs` |
| OZ export | `oz::tests::exports_native_pdf_from_cross_origin_oz_frame` with Chromium |
| PDF page text | `tests/pdf.rs` with `tests/fixtures/sample.pdf` |

## Deliberate differences

- **OZ export uses Chromium.** The executable opens a temporary browser profile
  and invokes the native Save → Adobe PDF command.
- **PDF text uses `lopdf`.** Text extraction may differ from Python `pypdf` on
  unusual fonts or layouts. The fixture covers the normal text path.
- **`list_learning_items(detail=true)` returns the raw provider modules
  without applying week/query/type filters.** The compact/default path is fully
  ported; this legacy detail path is intentionally left unfiltered in the
  experimental Rust core.
- **HTTP is the default transport.** Login and session refresh read Chrome
  profile cookies. OZ export uses a temporary local CDP connection.
- **MCP protocol version is `2024-11-05`.** The Rust server exposes 31 tools.
  The legacy FastMCP client accepted the server before FastMCP was removed;
  current clients use raw stdio JSON-RPC.
- **Single-account process.** Like the Python service, caches and the saved
  session assume one account per process.

## Preserved quirks

- `observed_at` + `result` envelope, the `items` / `complete` / `has_more` /
  `next` / `next_param` list envelope, and Asia/Seoul `_seoul` date fields.
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
