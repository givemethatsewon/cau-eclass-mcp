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

일반 API 조회는 알려진 상대경로만 허용하며, 페이지네이션에서 타 origin으로 넘어가지 않는다. 파일 전송은 Canvas 파일 endpoint에서 받은 파일만 저장한다. 인증값은 tool 응답에서 제거하고 파일명은 정규화한다. 과제 제출은 opt-in과 준비 계획이 모두 필요하고 직렬화한다. 실 변경 후 오류가 나면 계획을 재사용할 수 없게 하며 자동 재시도하지 않는다.

미지원 기능은 반환값과 README에서 명시한다. 특히 메타데이터 content_type=mp4와 실제 영상 다운로드 성공을 구별한다. 실제 제출 코드는 mock 테스트만 통과했으므로 검증 완료로 표시하지 않는다.
