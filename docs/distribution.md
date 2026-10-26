# 배포와 참고한 설계

## 적용한 패턴

- [Microsoft Playwright MCP](https://github.com/microsoft/playwright-mcp): 기존 브라우저 연결과 환경변수/CLI 옵션을 명시적으로 구분. 연결한 사용자 브라우저는 서버가 소유하지 않음.
- [uv tools](https://docs.astral.sh/uv/guides/tools/): 프로젝트 checkout 없이 wheel을 독립 환경에 설치하고 실행. 저장소 개발 환경과 사용자 설치 환경을 분리.
- [MCP Registry 배포 요구사항](https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/server-json/official-registry-requirements.md): 패키지 게시와 MCP Registry 등록을 별도 단계로 취급. 검증하지 않은 패키지명/네임스페이스로 등록 완료를 주장하지 않음.
- [Playwright CLI SKILL.md](https://github.com/microsoft/playwright-cli/blob/main/skills/playwright-cli/SKILL.md): 짧은 시작 흐름과 실제 도구 호출 중심의 안내. 이 프로젝트 스킬에는 과목 선택 → 조회 → 파일 작업 흐름과 제출 경계를 적용.
- [Agent Skills 형식](https://agentskills.io/specification): frontmatter의 name/description과 짧은 작업 지침. 스킬은 MCP 연결을 대신하지 않으며 권한을 확대하지 않음.

코드는 참고 프로젝트에서 복사하지 않았습니다. 이 저장소의 브라우저 어댑터·서비스·CLI에 위 패턴을 적용했습니다.

## 로컬 릴리스 만들기

```sh
uv sync --frozen --extra cdp
uv run pytest -q
uv run ruff check src tests scripts
uv build --no-sources
uv run python scripts/check_dist.py
```

`dist/`의 wheel은 사용자 설치용, tar.gz는 소스 배포용입니다. wheel에는 런타임 코드와 SKILL.md를 포함합니다. 분석 원본, 학습자료, 다운로드, 실제 계정 테스트 결과, 개인 MCP 설정은 포함하지 않습니다. 설치 시 Python 의존성을 받기 위한 네트워크 연결이 필요합니다.

깨끗한 별도 환경에서 다음을 확인하세요.

```sh
uv tool install --python 3.12 --from ./dist/cau_eclass_mcp-0.2.0-py3-none-any.whl --with 'playwright>=1.60,<2' cau-eclass-mcp
eclass-mcp --version
eclass-mcp --browser cdp config --client claude
eclass-mcp skill
```

Aside만 사용할 경우 `--with playwright...`는 생략합니다. 패키지를 다시 설치하거나 위치를 변경했다면 `config` 출력을 다시 생성하세요. 출력되는 Python 경로는 설치된 가상환경의 절대 경로입니다. 임시 `uvx` 환경의 경로를 장기 설정에 넣지 않도록, 설정 생성에는 영구 `uv tool install` 또는 checkout의 `.venv`를 사용하세요.

## 공개 게시 전 남은 설정

현재 Git remote·PyPI 게시 대상·MCP Registry 소유 네임스페이스·배포 라이선스가 정해지지 않았습니다. 로컬 설치 가능한 산출물과 CI까지만 준비하며, 공개 배포나 등록은 수행하지 않습니다. 라이선스가 결정되기 전에는 오픈소스 라이선스가 부여된 것으로 간주하지 마세요.

게시 시에는 소유권을 확인한 패키지/저장소 URL을 프로젝트 메타데이터에 추가하고 라이선스를 포함하세요. 그다음 PyPI에 게시한 정확한 버전으로 clean install을 확인하고, 공식 스키마에 맞는 `server.json`을 생성·검증한 뒤 MCP Registry에 등록합니다. PyPI에 아직 게시하지 않은 현재 버전을 `uvx cau-eclass-mcp`로 설치할 수 있다고 안내하지 않습니다.

GitHub Actions는 테스트·빌드 산출물만 생성하며 자동 게시 권한이나 토큰을 요구하지 않습니다.
