# 구조와 브라우저 경계

2026-11-12 갱신. 이 서버는 **로그인만 Chrome 프로필, 이후 읽기·다운로드·제출은 HTTP**를 기본으로 한다. remote debugging 포트나 CDP 연결을 사용하지 않는다.

```text
MCP client
  → Rust stdio server (28 tools)
  → EClassService (pagination, dates, briefing, validation)
  → HttpTransport (TTL cache + single-flight, cookie jar)
      ├ Canvas /api/v1 + LearningX /learningx/api/v1
      └ session.json (0600)
  → optional OZ helper (Playwright, only download_syllabus_pdf)
```

## HTTP 우선 전송

1. 사용자가 `eclass-mcp login` 또는 `onboard`로 전용 Chrome 프로필에 로그인한다.
2. 서버가 Chrome 프로필의 쿠키 DB를 직접 읽어 `.cau.ac.kr` 쿠키를 `session.json`에 저장한다. HttpOnly 쿠키를 포함하며, remote debugging 포트는 열지 않는다.
3. 이후 `HttpTransport`가 쿠키 jar와 `xn_api_token` Bearer 헤더로 Canvas/LearningX JSON을 직접 요청한다.
4. 401·로그인 HTML을 만나면 저장된 Chrome 프로필에서 쿠키를 다시 읽는다. 프로필이 없거나 잠겨 있으면 `eclass-mcp login`을 안내한다.
5. LearningX 401은 강좌 페이지의 LTI 폼을 저장된 쿠키로 다시 POST해 토큰을 스스로 발급한다. 그래도 실패하면 `auth refresh --via aside`가 로그인된 Aside 브라우저에서 토큰만 회수한다.

이 구조의 핵심 이점:

- 실행 중 브라우저·Playwright·페이지 컨텍스트가 필요 없다.
- 화면 리소스(JS/CSS/이미지/폰트)를 로드하지 않는다.
- 동일 응답을 TTL 캐시와 single-flight로 재사용한다.
- 과목별 공지 조회를 최대 4개까지 병렬화한다.

## 브라우저 경계는 OZ PDF 하나

`download_syllabus_pdf`만 브라우저 UI가 필요하다. OZ 뷰어는 `/oz80/server`에서 보고서 데이터를 받고, Save → Adobe PDF에서 `OZJSViewer_export.js`를 지연 로드해 브라우저 안에서 PDF를 만든다. 서버에서 바로 받을 PDF URL은 없다.

Rust MCP는 이 도구를 받으면 선택적 `eclass-oz-export` 헬퍼를 subprocess로 실행한다. 헬퍼는 같은 `session.json`의 쿠키를 Playwright 컨텍스트에 주입하고, OZ Save 대화상자를 조작해 PDF를 만든 뒤 경로·크기·해시·페이지 텍스트를 JSON으로 돌려준다. Rust는 반환 경로가 다운로드 디렉터리 안인지, `%PDF-` 헤더와 크기가 유효한지 다시 검증한다.

과제 제출은 브라우저가 필요 없다. Canvas 업로드 티켓 → multipart 업로드(쿠키·인증 헤더 없이) → 제출 POST → readback 검증을 Rust HTTP로 수행한다. 기본 비활성화이며 `ECLASS_ENABLE_SUBMISSION=1`과 준비 계획·확인 문자열이 필요하다.

## 캐시

`HttpTransport` 내부 `TtlCache`가 `(path, learningx, html)` 키로 GET 응답을 캐시한다. 기본 TTL은 20초이며 `ECLASS_CACHE_TTL=0`으로 끌 수 있다. 동시에 같은 키를 요청하면 하나의 in-flight task를 공유한다. 세션을 재수집하거나 쓰기 후에는 캐시를 비워 제출 결과를 이전 응답으로 오판하지 않는다.

## 보안 경계

- 비밀번호·OTP는 수집하지 않는다. 로그인은 사용자 Chrome 프로필에서 직접 한다.
- 세션 파일은 `.cau.ac.kr` 쿠키만 담고 `0600`으로 저장한다. 서명 URL·본문·비밀번호는 저장하지 않는다.
- 일반 조회는 같은 origin의 알려진 상대경로만 허용한다. 페이지네이션의 cross-origin 링크는 거부한다.
- 다운로드는 허용 호스트만 따르며, 리다이렉트마다 목적지를 재검증한다.
- LearningX Bearer 토큰은 LearningX 요청에만 붙이고, 다운로드 호스트에는 보내지 않는다.
- OZ 헬퍼는 stdout으로 PDF 결과 JSON만 출력하고, 오류 문자열에서 URL을 제거한다.
- 학습자료·공지 본문은 신뢰하지 않는 데이터로 취급한다.

## 구성

- `rust/src/session.rs`: `session.json`, Chrome 프로필 쿠키 import, 온보딩 상태
- `rust/src/http.rs`: HTTP GET, 리다이렉트, 다운로드, TTL 캐시
- `rust/src/service.rs`: 페이지네이션, briefing, 자료, PDF, 다운로드, 제출, OZ 헬퍼 호출
- `rust/src/mcp.rs`: stdio JSON-RPC MCP 서버 28개 도구
- `src/eclass_mcp/oz_export.py`: OZ PDF 전용 Python/Playwright 헬퍼
- `src/eclass_mcp/session.py`: 헬퍼가 읽는 세션 형식
- `skills/cau-eclass/SKILL.md`: 에이전트 스킬 원본

Python 패키지에는 MCP 서버, FastMCP, CDP, 전체 HTTP 서비스가 없다. OZ 헬퍼와 세션 로더만 남긴다.
