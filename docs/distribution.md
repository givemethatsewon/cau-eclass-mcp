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

## 게시 설정

- 공개 저장소: `givemethatsewon/cau-eclass-mcp`
- PyPI 패키지: `cau-eclass-mcp`
- 요청한 PyPI 계정명: `givemehtatsewon` (GitHub 계정명과 철자가 다름)
- 라이선스: 프로젝트 전용 `LicenseRef-CAU-EClass-Noncommercial-1.0`. 상업적 사용 금지, 기관 유형에 따른 예외 없음. LICENSE와 NOTICE를 배포본에 포함.

PyPI 계정 생성·이메일 인증·2FA 설정 후 [Publishing](https://pypi.org/manage/account/publishing/)에서 pending publisher를 등록합니다.

| 필드 | 값 |
|---|---|
| PyPI Project Name | `cau-eclass-mcp` |
| Owner | `givemethatsewon` |
| Repository name | `cau-eclass-mcp` |
| Workflow name | `release.yml` |
| Environment name | `pypi` |

계정 username은 패키지의 GitHub Owner 필드가 아닙니다. GitHub Owner는 저장소 소유 계정의 정확한 철자를 사용합니다. API 토큰이나 PyPI 비밀번호를 GitHub Secrets에 저장하지 않습니다.

검증 완료된 커밋에 `v0.2.0` 태그를 만든 후 GitHub Actions의 **Publish to PyPI**를 해당 태그로 수동 실행합니다. 워크플로는 버전 일치 검사·테스트·빌드·배포본 검사를 수행하고 `pypi` 환경에서 OIDC Trusted Publishing으로 게시합니다. 일반 push/PR/릴리스 생성만으로 게시하지 않습니다.

PyPI 게시 전에는 `uvx cau-eclass-mcp`로 현재 버전을 설치할 수 있다고 안내하지 않습니다. 최초 게시 후 설치 readback과 PyPI 메타데이터·해시를 확인한 뒤 문서를 갱신합니다. MCP Registry 등록은 PyPI 게시 후 별도 단계입니다.
