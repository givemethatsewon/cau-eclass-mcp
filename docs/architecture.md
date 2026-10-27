# FastMCP 선택과 구성

2026-10-26 검토. 독립 패키지 `fastmcp==4.1.4`를 사용하며 `uv.lock`으로 전이 의존성도 고정한다. 공식 Python SDK에 과거 포함됐던 동명의 클래스와 혼동하지 않는다.

선택 이유:
- Python 타입/Pydantic으로 입력 범위와 MCP 스키마를 함께 정의한다.
- lifespan에서 Aside 클라이언트를 열고 종료 시 정리한다.
- FastMCP Client로 메모리 내 계약 테스트와 실제 stdio 서버 테스트를 모두 수행한다.
- MCP Client 기능으로 기존 `aside mcp`의 공식 `repl` 도구를 호출한다. 별도 비공식 CDP endpoint를 사용하지 않는다.

근거: [lifespan](https://gofastmcp.com/servers/lifespan), [transports](https://gofastmcp.com/clients/transports), [testing](https://gofastmcp.com/servers/testing). PyPI 버전 확인과 실제 설치·테스트를 모두 수행했다.

```text
Codex / MCP client
  → FastMCP stdio server (server.py: tools and schema)
  → EClassService (service.py: pagination, dates, plans, validation)
  → AsideTransport (transport.py: persistent MCP client, serialized REPL)
  → existing logged-in e-Class tab
      ├ Canvas: same-origin session + CSRF on writes
      └ LearningX: xn_api_token → Authorization Bearer inside browser only
```

FastMCP는 대학 인증이나 파일 전송을 대신 구현해주지 않는다. 이 로직은 service/transport로 분리했다. 현재 Aside 의존성은 로그인 구현과 쿠키 외부 추출을 피하는 대신, 로컬 브라우저와 CLI가 실행 중이어야 한다는 제약을 가진다. 독립 HTTP 세션 adapter는 아직 없다.

일반 API 조회는 알려진 상대경로만 허용하며, 페이지네이션에서 타 origin으로 넘어가지 않는다. 파일 전송은 Canvas 파일과 Commons가 실제 제공한 네이티브 원본 PDF를 지원한다. `catalog.py`가 자료 ID·검색·간결한 목록을, `documents.py`가 PDF 검증과 페이지별 이어 읽기를 담당한다. 인증값은 tool 응답에서 제거하고 파일명은 정규화한다. PDF 본문은 페이지 전체에서 URL을 정리한 뒤 분할하며 MCP 응답 단계에서 청크를 다시 변형하지 않는다. 과제 제출은 opt-in과 준비 계획이 모두 필요하고 직렬화한다. 실 변경 후 오류가 나면 계획을 재사용할 수 없게 하며 자동 재시도하지 않는다.

미지원 기능은 반환값과 README에서 명시한다. 특히 메타데이터 content_type=mp4와 실제 영상 다운로드 성공을 구별한다. 실제 제출 코드는 mock 테스트만 통과했으므로 검증 완료로 표시하지 않는다.

## 0.2.0 배포 구조

`cli.py`는 설치 진단·설정 출력·스킬 출력과 stdio 서버 시작을 분리한다. `config.py`가 `ECLASS_BROWSER`에 따라 Aside 또는 `CDPTransport`를 선택한다. `browser_scripts.py`의 Canvas/LearningX 요청·영상 메타데이터·제출 스크립트를 두 어댑터가 공유한다.

CDP는 optional `cdp` extra의 Playwright Python 연결 API를 사용한다. 브라우저 시작/종료를 소유하지 않고 기존 로컬 디버깅 세션에 붙는다. 다운로드는 HTTP 스트리밍으로 크기를 제한하며 리다이렉트마다 목적지·쿠키 범위를 검사한다. CDP 모드에서 쿠키는 다운로드 클라이언트 메모리로 전달되므로, 과거 Aside 구현에만 해당했던 '브라우저 내부에서만 처리' 설명과 구분한다. 파일이나 MCP 응답으로 내보내지 않는다.

개발 checkout과 설치된 wheel 모두 같은 CLI를 제공한다. 스킬 원본은 `skills/cau-eclass/SKILL.md` 하나로 유지하고 wheel 빌드 시 패키지 리소스에 포함한다. 공개 게시 준비 상태와 참고 근거는 [배포 문서](distribution.md)에 기록한다.
