# 배포와 참고한 설계

## 적용한 패턴

- [Microsoft Playwright MCP](https://github.com/microsoft/playwright-mcp): 기존 브라우저 연결과 환경변수/CLI 옵션을 명시적으로 구분. 연결한 사용자 브라우저는 서버가 소유하지 않음.
- [uv tools](https://docs.astral.sh/uv/guides/tools/): 프로젝트 checkout 없이 wheel을 독립 환경에 설치하고 실행. 저장소 개발 환경과 사용자 설치 환경을 분리.
- [MCP Registry 배포 요구사항](https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/server-json/official-registry-requirements.md): 패키지 게시와 MCP Registry 등록을 별도 단계로 취급. 검증하지 않은 패키지명/네임스페이스로 등록 완료를 주장하지 않음.
- [Playwright CLI SKILL.md](https://github.com/microsoft/playwright-cli/blob/main/skills/playwright-cli/SKILL.md): 짧은 시작 흐름과 실제 도구 호출 중심의 안내. 이 프로젝트 스킬에는 과목 선택 → 조회 → 파일 작업 흐름과 제출 경계를 적용.
- [Agent Skills 형식](https://agentskills.io/specification): frontmatter의 name/description과 짧은 작업 지침. 스킬은 MCP 연결을 대신하지 않으며 권한을 확대하지 않음.

코드는 참고 프로젝트에서 복사하지 않았습니다. 이 저장소의 브라우저 어댑터·서비스·CLI에 위 패턴을 적용했습니다.

## 로컬 릴리스 만들기

Rust MCP 릴리스:

```sh
cd rust
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

Python OZ 헬퍼 릴리스:

```sh
uv sync --frozen --extra oz
uv run pytest -q
uv run ruff check src tests scripts
uv build --no-sources
uv run python scripts/check_dist.py
uv run python scripts/check_install.py
```

`dist/`의 wheel은 OZ PDF 헬퍼 설치용, tar.gz는 소스 배포용입니다. wheel에는
`eclass_mcp/oz_export.py`, `session.py`, `errors.py`와 SKILL.md만 포함합니다.
MCP 서버·FastMCP·CDP·전체 HTTP 서비스는 포함하지 않습니다.

깨끗한 별도 환경에서 다음을 확인합니다.

```sh
uv tool install --python 3.12 --from ./dist/cau_eclass_mcp-0.5.0-py3-none-any.whl   --with 'playwright>=1.60,<2' cau-eclass-mcp
eclass-oz-export --help
```

Rust MCP는 GitHub Releases의 `eclass-mcp` 바이너리를 사용합니다. OZ PDF를 쓸
때만 `ECLASS_OZ_HELPER`에 `eclass-oz-export` 절대 경로를 지정합니다.

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

검증 완료된 커밋에 `v0.4.0` 태그를 만든 후 GitHub Actions의 **Publish to PyPI**를 해당 태그로 수동 실행합니다. 워크플로는 버전 일치 검사·테스트·빌드·배포본 검사를 수행하고 `pypi` 환경에서 OIDC Trusted Publishing으로 게시합니다. 일반 push/PR/릴리스 생성만으로 게시하지 않습니다.

2026-10-26에 `0.2.0`을 PyPI에 게시했습니다. 0.5.0은 OZ 전용 헬퍼로 패키지 범위를 축소했습니다. PyPI 직접 설치 후 버전·번들 스킬·생성 설정의 실행·doctor 실패 계약을 검증했고, PyPI의 wheel/sdist SHA256과 GitHub Actions 산출물이 일치함을 확인했습니다. 비상업 라이선스 메타데이터도 확인했습니다. MCP Registry는 아직 등록하지 않았습니다.

패키지명과 실행 명령이 다르므로 일회성 실행은 `uvx --from 'cau-eclass-mcp[oz]==0.5.0' eclass-oz-export --help`처럼 사용합니다. 장기 MCP 설정 생성에는 위의 `uv tool install`을 사용하세요.
