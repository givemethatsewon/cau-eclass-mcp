# Rust porting notes

Source of truth: the Python `src/eclass_mcp` package in this repository.
Target: `rust/src` (`eclass_core` library + `eclass-mcp-rs` binary).

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
| `download_syllabus_pdf` | `BrowserOnly` error |
| `prepare_submission` / `submit_assignment` | `BrowserOnly` error |

## Test mapping

| Python test area | Rust test |
|---|---|
| `tests/test_session.py` | `src/session.rs` unit tests |
| `tests/test_http_transport.py` | `src/http.rs` wiremock tests |
| `tests/test_cache.py` | cache behavior inside `src/http.rs` tests |
| `tests/test_catalog.py` | `src/catalog.rs` unit tests |
| `tests/test_briefing.py` | `tests/service.rs` briefing test |
| `tests/test_materials.py` | `tests/service.rs` Commons PDF test |
| `tests/test_cdp_browser.py` | no direct Rust test; Chrome profile cookie import was manually checked with a temporary profile |
| PDF page text | `tests/pdf.rs` with `tests/fixtures/sample.pdf` |

## Deliberate differences

- **Browser-only features are explicit errors.** OZ PDF export, submission
  preparation, and actual submission stay in the Python/browser build.
- **PDF text uses `lopdf`.** Text extraction may differ from Python `pypdf` on
  unusual fonts or layouts. The fixture covers the normal text path.
- **`list_learning_items(detail=true)` returns the raw provider modules
  without applying week/query/type filters.** The compact/default path is fully
  ported; this legacy detail path is intentionally left unfiltered in the
  experimental Rust core.
- **Only HTTP transport is available.** `ECLASS_BROWSER=aside` remains a Python
  feature. Rust reads Chrome profile cookies for the one-time `login` and
  session refresh; no remote debugging port is opened.
- **MCP protocol version is `2024-11-05`.** The FastMCP Python client accepted
  the Rust server and listed all 27 tools. Rust adds
  `get_onboarding_status` to the Python 26-tool surface.
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

Results at the time of writing: 20 unit tests, 2 PDF integration tests, 6
service integration tests, clippy clean, release build successful.
