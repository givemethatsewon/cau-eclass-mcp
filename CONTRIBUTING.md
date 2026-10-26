# 개발

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

배포 산출물 검증은 [배포 문서](docs/distribution.md)를 따릅니다. `scripts/live_*.py`는 이전 실계정 조사 도구이며 일반 CI에서 실행하지 않습니다.
