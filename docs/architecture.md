# 구조와 전송 선택

2026-11-12 갱신. 이 서버는 **로그인만 브라우저, 이후 읽기는 HTTP**를 기본으로 한다. 브라우저 자동화는 OZ 계획서 PDF 내보내기와 실제 과제 제출처럼 UI가 필요한 작업에만 남긴다.

```text
MCP client
  → FastMCP stdio server (server.py: tool schema)
  → EClassService (service.py: pagination, dates, briefing, validation)
  → CachingTransport (cache.py: 20s TTL, single-flight)
  → HTTPTransport (http_transport.py: cookie jar, JSON, downloads)
      ├ Canvas /api/v1 + LearningX /learningx/api/v1
      └ saved session (session.py: 0600 cookie file)
  → browser fallback (only OZ PDF export / submission)
```

## 왜 FastMCP인가

독립 패키지 `fastmcp==4.1.4`를 사용하고 `uv.lock`으로 전이 의존성을 고정한다. Python 타입/Pydantic으로 입력 범위와 MCP 스키마를 함께 정의하고, lifespan에서 transport를 열고 닫으며, FastMCP Client로 메모리 내 계약 테스트와 실제 stdio 테스트를 모두 수행할 수 있다.

근거: [lifespan](https://gofastmcp.com/servers/lifespan), [transports](https://gofastmcp.com/clients/transports), [testing](https://gofastmcp.com/servers/testing).

## HTTP 우선 전송

초기 버전은 Aside MCP의 `repl` 또는 CDP 페이지에서 `fetch`를 실행했다. 이 방식은 로그인·쿠키를 브라우저 밖으로 내보내지 않는다는 장점이 있지만, 요청마다 브라우저 프로세스와 페이지 컨텍스트가 필요하고 응답이 JSON으로 직렬화되는 비용이 크다.

0.4.0은 다음처럼 바꿨다.

1. 사용자가 `eclass-mcp login`으로 전용 Chromium 프로필에 로그인한다.
2. 서버가 CDP `Storage.getCookies`로 `.cau.ac.kr` 쿠키를 한 번 읽어 `session.json`에 저장한다. HttpOnly 세션 쿠키를 포함한다.
3. 이후 `HTTPTransport`가 쿠키 jar와 `xn_api_token` Bearer 헤더로 Canvas/LearningX JSON을 직접 요청한다.
4. 401·로그인 HTML을 만나면 열려 있는 로그인 브라우저에서 쿠키를 다시 읽는다. 브라우저가 없으면 `eclass-mcp login`을 안내한다.

이 구조의 핵심 이점:

- 실행 중 브라우저·Playwright·페이지 컨텍스트가 필요 없다.
- 화면 리소스(JS/CSS/이미지/폰트)를 로드하지 않는다.
- 동일 응답을 TTL 캐시와 single-flight로 재사용한다.
- 과목별 공지 조회를 최대 4개까지 병렬화한다.

## 캐시

`cache.py`는 `(path, learningx, html)` 키로 GET 응답을 캐시한다. 기본 TTL은 20초이며 `ECLASS_CACHE_TTL=0`으로 끌 수 있다. 동시에 같은 키를 요청하면 하나의 in-flight task를 공유한다. 반환값은 깊은 복사해 서비스가 목록을 변형해도 캐시가 오염되지 않는다. 세션을 재수집하면 캐시를 비운다. 쓰기 후에는 서비스가 `invalidate()`로 캐시를 비워 제출 결과를 이전 응답으로 오판하지 않는다.

## 브라우저 경계

다음 두 작업은 아직 브라우저 UI가 필요하다.

- `download_syllabus_pdf`: OZ 뷰어의 Save → Adobe PDF 대화상자와 다운로드 이벤트.
- `submit_assignment`: 기존 검증된 브라우저 업로드·CSRF 경로.

`HTTPTransport`는 이 두 메서드만 lazy하게 `CDPTransport`(기본) 또는 `AsideTransport`로 위임한다. 로그인 브라우저가 닫혀 있으면 명확한 오류를 반환하고, 일반 조회·다운로드는 계속 HTTP로 동작한다.

## 보안 경계

- 비밀번호·OTP는 수집하지 않는다. 로그인은 사용자 브라우저에서 직접 한다.
- 세션 파일은 `.cau.ac.kr` 쿠키만 담고 `0600`으로 저장한다. 서명 URL·본문·비밀번호는 저장하지 않는다.
- 일반 조회는 같은 origin의 알려진 상대경로만 허용한다. 페이지네이션의 cross-origin 링크는 거부한다.
- 다운로드는 `eclass3.cau.ac.kr`, `ocs.cau.ac.kr`, `cau-cms-object.cdn.gov-ntruss.com` 등 허용 호스트만 따르며, 리다이렉트마다 목적지를 재검증한다.
- LearningX Bearer 토큰은 LearningX 요청에만 붙이고, 다운로드 호스트에는 보내지 않는다.
- 도구 응답의 URL은 민감한 쿼리를 제거하고, 인증값이 담긴 키는 제거한다.
- 학습자료·공지 본문은 신뢰하지 않는 데이터로 취급한다.

## 0.3.0과의 호환성

`ECLASS_BROWSER=aside` 또는 `cdp`를 지정하면 기존 브라우저 어댑터를 그대로 사용한다. 환경 변수를 지정하지 않으면 저장된 세션이 있을 때 `http`, 없고 Aside가 설치되어 있으면 `aside`, 그 외에는 `http`를 선택한다. 기존 사용자는 설정을 바꾸지 않아도 동작하지만, `login` 후 HTTP 모드로 전환하면 더 가볍다.

## 배포 구조

`cli.py`는 설치 진단·세션 로그인·설정 출력·스킬 출력과 stdio 서버 시작을 분리한다. `config.py`가 전송을 선택한다. `session.py`가 쿠키 저장·CDP 수집·갱신을, `http_transport.py`가 HTTP 요청과 Commons/영상 다운로드를, `catalog.py`가 자료 ID·검색·간결한 목록을, `documents.py`가 PDF 검증과 페이지별 이어 읽기를 담당한다.

CDP와 Aside 어댑터는 `browser_scripts.py`의 페이지 내 스크립트를 공유한다. 스킬 원본은 `skills/cau-eclass/SKILL.md` 하나로 유지하고 wheel 빌드 시 패키지 리소스에 포함한다.
