# 구조와 전송 선택

2026-11-12 갱신. 이 서버는 **로그인만 Chrome 프로필, 이후 읽기는 HTTP**를 기본으로 한다. remote debugging 포트나 CDP 연결을 사용하지 않는다.

```text
MCP client
  → FastMCP / Rust stdio server
  → EClassService (pagination, dates, briefing, validation)
  → CachingTransport (TTL + single-flight)
  → HTTPTransport (cookie jar, JSON, downloads)
      ├ Canvas /api/v1 + LearningX /learningx/api/v1
      └ saved session (session.json: 0600)
  → browser helper (only OZ PDF export / submission)
```

## HTTP 우선 전송

1. 사용자가 `eclass-mcp login`으로 전용 Chrome 프로필에 로그인한다.
2. 서버가 Chrome 프로필의 쿠키 DB를 직접 읽어 `.cau.ac.kr` 쿠키를 `session.json`에 저장한다. HttpOnly 쿠키를 포함하며, remote debugging 포트는 열지 않는다.
3. 이후 `HTTPTransport`가 쿠키 jar와 `xn_api_token` Bearer 헤더로 Canvas/LearningX JSON을 직접 요청한다.
4. 401·로그인 HTML을 만나면 저장된 Chrome 프로필에서 쿠키를 다시 읽는다. 프로필이 없거나 잠겨 있으면 `eclass-mcp login`을 안내한다.

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

일반 조회·다운로드는 HTTP로 동작한다. OZ·제출을 쓰려면 Playwright 기반 브라우저 도우미가 필요하며, Python 전체 기능 빌드에서 제공한다. Rust 코어는 이 두 도구를 명시적인 `BrowserOnly` 오류로 반환한다.

## 보안 경계

- 비밀번호·OTP는 수집하지 않는다. 로그인은 사용자 Chrome 프로필에서 직접 한다.
- 세션 파일은 `.cau.ac.kr` 쿠키만 담고 `0600`으로 저장한다. 서명 URL·본문·비밀번호는 저장하지 않는다.
- 일반 조회는 같은 origin의 알려진 상대경로만 허용한다. 페이지네이션의 cross-origin 링크는 거부한다.
- 다운로드는 `eclass3.cau.ac.kr`, `ocs.cau.ac.kr`, `cau-cms-object.cdn.gov-ntruss.com` 등 허용 호스트만 따르며, 리다이렉트마다 목적지를 재검증한다.
- LearningX Bearer 토큰은 LearningX 요청에만 붙이고, 다운로드 호스트에는 보내지 않는다.
- 도구 응답의 URL은 민감한 쿼리를 제거하고, 인증값이 담긴 키는 제거한다.
- 학습자료·공지 본문은 신뢰하지 않는 데이터로 취급한다.

## 0.3.0과의 호환성

`ECLASS_BROWSER=aside`는 기존 Aside 어댑터로 유지한다. `ECLASS_BROWSER`를 지정하지 않으면 저장된 세션이 있을 때 `http`, 없고 Aside가 설치되어 있으면 `aside`, 그 외에는 `http`를 선택한다. Chrome 프로필 로그인 후 HTTP 모드로 전환하면 더 가볍다.

## 배포 구조

`cli.py`는 설치 진단·세션 로그인·설정 출력·스킬 출력과 stdio 서버 시작을 분리한다. `config.py`가 전송을 선택한다. `session.py`가 Chrome 프로필 쿠키 저장·갱신을, `http_transport.py`가 HTTP 요청과 Commons/영상 다운로드를, `catalog.py`가 자료 ID·검색·간결한 목록을, `documents.py`가 PDF 검증과 페이지별 이어 읽기를 담당한다.

Rust 코어는 `rust/src/session.rs`에서 같은 `session.json` 형식을 읽고, `http.rs`·`service.rs`·`catalog.rs`·`documents.rs`·`mcp.rs`로 같은 데이터 흐름을 구현한다. 스킬 원본은 `skills/cau-eclass/SKILL.md` 하나로 유지하고 wheel 빌드 시 패키지 리소스에 포함한다.
