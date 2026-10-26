# 개발

README·CHANGELOG·CONTRIBUTING·SECURITY 및 `docs/`만 변경한 push/PR은 CI를 건너뜁니다. 코드·설정·배포 스킬(`skills/`) 변경이 포함되면 CI를 실행합니다. 필요하면 Actions에서 수동 실행할 수 있습니다. 태그 push와 PyPI 게시 워크플로의 검증은 유지됩니다.

Python 3.12 이상과 uv를 사용합니다.

```sh
uv sync --frozen --extra cdp
uv run pytest -q
uv run ruff check src tests scripts
uv build --no-sources
uv run python scripts/check_dist.py
```

실제 CDP 테스트는 `ECLASS_TEST_CHROME`에 Chrome/Chromium 실행 파일 경로를 설정하고 `uv run pytest tests/test_cdp_browser.py -q`로 실행합니다. 테스트는 임시 프로필과 synthetic 사이트 응답을 사용하며 작업이 만든 브라우저만 종료합니다. 실제 계정 테스트는 본인에게 허용된 자료로 실행하고, 과제 제출·업로드·영상 재생·진도 갱신은 검증 명목으로 실행하지 마세요.

`browser_scripts.py`는 두 어댑터가 공유하는 페이지 내 요청 코드입니다. 서비스의 데이터 해석/제출 계획과 브라우저 연결/파일 전송 로직을 분리하세요. 인증값·서명 URL을 예외, fixture, 로그, PR에 넣지 마세요. 테스트 데이터는 가상 계정·가상 ID를 사용합니다. MCP stdout은 프로토콜 전용입니다.

배포 산출물 검증은 [배포 문서](docs/distribution.md)를 따릅니다. 연동 변경은 [연동 계약](docs/integration.md)을 참고하세요. 원본 조사 자료·실계정 응답·개인용 조사 스크립트는 커밋하지 않습니다.
