# 개발

README·CHANGELOG·CONTRIBUTING·SECURITY 및 `docs/`만 변경한 push/PR은 CI를 건너뜁니다. 코드·설정·배포 스킬(`skills/`) 변경이 포함되면 CI를 실행합니다. 필요하면 Actions에서 수동 실행할 수 있습니다. 태그 push와 PyPI 게시 워크플로의 검증은 유지됩니다.

Rust가 primary MCP 서버이고, Python은 OZ PDF 헬퍼만 담당합니다. Python 3.12 이상과 uv는 OZ 헬퍼를 수정할 때만 필요합니다.

```sh
# Rust MCP
cd rust
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release

# Python OZ helper
uv sync --frozen
uv run pytest -q
uv run ruff check src tests scripts
uv build --no-sources
uv run python scripts/check_dist.py
uv run python scripts/check_install.py
```

실제 OZ PDF 검증은 본인 계정의 강좌에서만 수행하고, 세션 파일·쿠키·PDF 본문을 fixture나 로그에 넣지 마세요. 실제 과제 제출·업로드·영상 재생·진도 갱신은 검증 명목으로 실행하지 마세요.

Rust 코어는 `rust/src/session.rs`(세션·쿠키), `http.rs`(HTTP·캐시), `service.rs`(자료·제출·OZ 헬퍼 호출), `mcp.rs`(stdio JSON-RPC)로 나뉩니다. Python `oz_export.py`는 OZ PDF만 담당하고, `session.py`는 Rust가 만든 세션을 읽습니다. 인증값·서명 URL을 예외, fixture, 로그, PR에 넣지 마세요. 테스트 데이터는 가상 계정·가상 ID를 사용합니다.

배포 산출물 검증은 [배포 문서](docs/distribution.md)를 따릅니다. 연동 변경은 [연동 계약](docs/integration.md)을 참고하세요. 원본 조사 자료·실계정 응답·개인용 조사 스크립트는 커밋하지 않습니다.
