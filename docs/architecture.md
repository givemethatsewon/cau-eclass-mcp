# 구조와 브라우저 경계

로그인은 전용 Chromium 프로필에서 진행하며 일반 조회·다운로드·제출은 HTTP를 사용한다. OZ 강의계획서 PDF 저장은 Rust 서버가 별도의 임시 Chromium 프로필을 열어 처리한다.

```text
MCP client
  → Rust stdio server (31 tools)
  → EClassService (pagination, dates, briefing, validation)
  → HttpTransport (TTL cache + single-flight, cookie jar)
      ├ Canvas /api/v1 + LearningX /learningx/api/v1
      └ session.json (0600)
  → Chromium CDP (temporary profile, only download_syllabus_pdf)
```

## HTTP 우선 전송

1. 사용자가 `eclass-mcp login` 또는 `onboard`로 전용 Chrome 프로필에 로그인한다.
2. 서버가 Chrome 프로필의 쿠키 DB를 직접 읽어 `.cau.ac.kr` 쿠키를 `session.json`에 저장한다. HttpOnly 쿠키를 포함하며, remote debugging 포트는 열지 않는다.
3. 이후 `HttpTransport`가 쿠키 jar와 `xn_api_token` Bearer 헤더로 Canvas/LearningX JSON을 직접 요청한다.
4. 401·로그인 HTML을 만나면 저장된 Chrome 프로필에서 쿠키를 다시 읽는다. 프로필이 없거나 잠겨 있으면 `eclass-mcp login`을 안내한다.
5. LearningX 401은 강좌 페이지의 LTI 폼을 저장된 쿠키로 다시 POST해 토큰을 스스로 발급한다. 그래도 실패하면 `auth refresh --via aside`가 로그인된 Aside 브라우저에서 토큰만 회수한다.

일반 HTTP 요청의 동작

- 저장된 세션으로 실행하며 브라우저 프로세스는 띄우지 않는다.
- 화면 리소스(JS/CSS/이미지/폰트)를 로드하지 않는다.
- 동일 응답을 TTL 캐시와 single-flight로 재사용한다.
- 과목별 공지 조회를 최대 4개까지 병렬화한다.

## 브라우저 경계는 OZ PDF 하나

`download_syllabus_pdf`만 브라우저 UI가 필요하다. OZ 뷰어는 `/oz80/server`에서 보고서 데이터를 받고, Save → Adobe PDF에서 `OZJSViewer_export.js`를 지연 로드해 브라우저 안에서 PDF를 만든다. 서버에서 바로 받을 PDF URL은 없다.

Rust는 임시 Chromium 프로필에 Canvas·OZ에 해당하는 세션 쿠키를 주입한다. CDP는 임시 브라우저의 로컬 연결로만 사용한다. 지정된 강좌 페이지와 OZ 프레임을 확인한 뒤 Save → Adobe PDF를 실행하고 다운로드가 끝나면 브라우저를 종료한다. 크기·형식·페이지를 검증한 파일만 다운로드 폴더에 저장하고 해시와 선택적 페이지 본문을 반환한다.

과제 제출은 브라우저가 필요 없다. Canvas 업로드 티켓 → multipart 업로드(쿠키·인증 헤더 없이) → 제출 POST → readback 검증을 Rust HTTP로 수행한다. 기본 비활성화이며 `ECLASS_ENABLE_SUBMISSION=1`과 준비 계획·확인 문자열이 필요하다.

## 캐시

`HttpTransport` 내부 `TtlCache`가 `(path, learningx, html)` 키로 GET 응답을 캐시한다. 기본 TTL은 20초이며 `ECLASS_CACHE_TTL=0`으로 끌 수 있다. 동시에 같은 키를 요청하면 하나의 in-flight task를 공유한다. 세션을 재수집하거나 쓰기 후에는 캐시를 비워 제출 결과를 이전 응답으로 오판하지 않는다.

## 보안 경계

- 비밀번호·OTP는 수집하지 않는다. 로그인은 사용자 Chrome 프로필에서 직접 한다.
- 세션 파일은 `.cau.ac.kr` 쿠키만 담고 `0600`으로 저장한다. 서명 URL·본문·비밀번호는 저장하지 않는다.
- 일반 조회는 같은 origin의 알려진 상대경로만 허용한다. 페이지네이션의 cross-origin 링크는 거부한다.
- 다운로드는 허용 호스트만 따르며, 리다이렉트마다 목적지를 재검증한다.
- LearningX Bearer 토큰은 LearningX 요청에만 붙이고, 다운로드 호스트에는 보내지 않는다.
- OZ DevTools 오류에서 브라우저 메시지와 URL을 노출하지 않는다. PDF가 검증되기 전에 최종 경로에 파일을 게시하지 않는다.
- 학습자료·공지 본문은 신뢰하지 않는 데이터로 취급한다.

## 구성

- `rust/src/session.rs`: `session.json`, Chrome 프로필 쿠키 import, 온보딩 상태
- `rust/src/http.rs`: HTTP GET, 리다이렉트, 다운로드, TTL 캐시
- `rust/src/service.rs`: 페이지네이션, briefing, 자료, PDF, 다운로드, 제출, OZ PDF 도구
- `rust/src/mcp.rs`: stdio JSON-RPC MCP 서버
- `rust/src/oz.rs`: OZ 프레임 선택, PDF 다운로드·검증
- `rust/src/cdp.rs`: 임시 Chromium 프로세스와 CDP 연결
- `skills/cau-eclass/SKILL.md`: 에이전트 스킬 원본
