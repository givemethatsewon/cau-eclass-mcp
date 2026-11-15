# Rust 전환 검토와 실험 결과

2026-11-12에 HTTP 우선 구조를 Rust로 이식해 아래 수치를 측정했다. 이후 `rust-v0.2.0`에서 OZ PDF 저장까지 Rust 실행파일에 통합했다.

## 실험 결과

`rust/`는 별도 Cargo 크레이트다. Python 0.4.0과 같은 `session.json`을 읽고,
같은 27개 도구에 `get_onboarding_status`를 더한 28개 MCP 도구를 제공한다.

| 항목 | Python 0.4.0 | Rust core |
|---|---:|---:|
| `tools/list`까지 시작 | 약 0.58~0.63초 | **약 0.04초** |
| `tools/list` CPU 시간 | 약 0.62초 | **<0.01초** |
| 최대 RSS | 약 106 MiB | **약 10.5 MiB** |
| 유휴 RSS | 약 106~111 MiB | **약 9.6 MiB** |
| 프로세스 수 | 1 | 1 |
| 릴리스 바이너리 | wheel + Python 환경 | **약 17 MiB 단일 실행 파일** |

대략 **시작 18~21배**, **메모리 약 90~91% 감소**다. Rust는 읽기·다운로드에
브라우저 프로세스를 띄우지 않는다. Python 브라우저 모드는 외부 Chrome 트리
약 674 MiB/7개 프로세스가 추가로 필요했다.

같은 `scripts/benchmark_server.py` 하네스, 가짜 세션, 실제 학교 네트워크
호출 없이 측정한 값이다. 실제 e-Class 지연을 측정한 것이 아니다. 네트워크
시간은 실제 도구 호출을 지배하므로, 시작·메모리 이득과 별개로 해석해야 한다.

이식 범위:

- 세션 파일 호환, Chrome 프로필 쿠키 수집, HttpOnly 쿠키
- HTTP GET·리다이렉트·페이지네이션·TTL 캐시·single-flight
- 강좌·todo·briefing·주차자료·공지·과제·마감·게시판·그룹
- Commons PDF/영상, Canvas 파일 다운로드
- PDF 검증·페이지별 텍스트(`lopdf`), 이어 읽기 offset
- MCP stdio JSON-RPC 28개 도구
- `login`, `onboard`, `logout`, `doctor`, `config`, `serve`

## OZ PDF 분석 결과

실제 강좌의 수업계획서를 Aside로 열어 읽기 전용으로 관찰했다.

- OZ 뷰어는 `rpt80.cau.ac.kr/oz80/ozViewer.jsp`를 iframe으로 열고,
  보고서 데이터는 `/oz80/server` XHR 4회로 가져온다.
- Save 대화상자에서 `Adobe PDF File(*.pdf)`를 선택하고 OK를 누르면
  `OZJSViewer_export.js`를 지연 로드한 뒤 브라우저 다운로드 `noname.pdf`가
  생성된다. **PDF 파일 자체를 서버에서 받는 요청은 없다.**
- 즉 PDF는 서버가 바로 내려주는 파일이 아니라 OZJSViewer의 내장 PDF writer가
  브라우저 안에서 보고서 데이터를 렌더링해 만든다. `/oz80/server` 응답에는
  PDF URL이 없다.

Rust는 설치된 Chromium 계열 브라우저를 임시 프로필로 띄우고 CDP를 통해 OZ 프레임의 Save → Adobe PDF를 실행한다. 브라우저가 생성한 원본을 받아 크기와 형식을 검증한 후 저장한다. 이 흐름에서 PDF 본문 추출은 `lopdf`가 맡는다.

배포 대상과 업데이트 방식은 [릴리스 문서](../rust/RELEASING.md)에 정리했다.

## Agent 온보딩 설계

e-Class에는 OAuth식 scope 동의가 없다. 온보딩은 **로그인 세션, LearningX
토큰, 강좌 접근을 확인하고 다음 행동을 안내하는 상태 머신**으로 설계한다.

`eclass-mcp onboard --client claude`:

1. 전용 Chromium 프로필을 열고 사용자가 직접 로그인한다.
2. Chrome 프로필 쿠키 DB를 읽어 세션을 저장한다.
3. 프로필 API, LearningX 토큰, 강좌 목록을 확인한다.
4. `schema_version`, `session_state`, `authenticated`, `learningx_token`,
   `courses_visible`, `ready`, `next_actions`를 JSON으로 출력한다.

`next_actions`는 사람용 문장 하나가 아니라 안정적인 **코드 + 메시지 +
선택적 명령**이다.

| 코드 | 의미 |
|---|---|
| `RUN_ONBOARD` | 저장된 세션 없음 |
| `SESSION_EXPIRED` | 세션은 있으나 만료/거부됨 |
| `CHECK_CONNECTION` | 네트워크·프로필 확인 실패 |
| `REFRESH_LEARNINGX_TOKEN` | Canvas는 되지만 LearningX 토큰이 없음 |
| `CHECK_ENROLLMENT` | 강좌 조회 실패 또는 0개 |
| `CONFIGURE_CLIENT` | 준비 완료, 클라이언트 설정 단계 |

`session_state`는 `missing`, `expired`, `unavailable`, `authenticated`를
구분한다. 단순히 `authenticated=false`로 뭉개지 않는다. MCP 서버는 세션이
없어도 시작하며, 이 상태에서 `get_onboarding_status`가 `missing`을 반환하고
다른 도구는 로그인 필요 오류를 낸다. 따라서 MCP 클라이언트 안에서도 첫 실행
안내가 가능하다. `get_onboarding_status`는 쿠키 값·Bearer 토큰·세션 파일
내용을 절대 포함하지 않으며, 이를 직렬화 테스트로 고정했다.

LearningX 토큰은 저장된 Canvas 쿠키로 강좌 페이지의 LTI 폼을 다시 POST해
발급한다. Canvas 쿠키가 만료됐으면 `auth refresh --via aside`가 로그인된
Aside 브라우저에서 토큰만 회수한다. 둘 다 실패하면
`REFRESH_LEARNINGX_TOKEN` 액션으로 남긴다.

세션 파일 위치는 macOS `~/Library/Application Support/cau-eclass/session.json`,
Linux `${XDG_CONFIG_HOME:-~/.config}/cau-eclass/session.json`, Windows
`%APPDATA%\cau-eclass\session.json`이며 `0600`으로 저장한다. `.cau.ac.kr`
쿠키만 담고 `logout`으로 삭제한다. OS 보안 저장소는 향후 개선 후보이지만,
현재는 소유자 전용 파일이 최소 조건이다.

온보딩은 권한을 확대하지 않는다. 기본 범위는 읽기 전용이고, 과제 제출은
별도 opt-in과 준비 계획이 필요한 실험 기능으로 남긴다.

## Go와 비교

Go도 여전히 가능하지만, Rust 구현이 이미 동작하고 측정까지 끝났으므로
언어를 다시 바꿀 이유는 크지 않다. Go의 장점은 표준 라이브러리와 단순한
동시성이고, Rust의 장점은 단일 바이너리 크기·메모리·기존 실험 코드다.
이 프로젝트의 병목은 학교 API이므로 두 언어의 성능 차이는 사용자 체감에
거의 영향이 없다. **중요한 결정은 언어가 아니라 배포·서명·온보딩·브라우저
경계다.**
