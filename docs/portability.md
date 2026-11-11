# Rust 전환 검토와 실험 결과

2026-11-12. 0.4.0의 HTTP 우선 구조를 `rust/`에 실제로 이식해 측정했다.
결론부터 말하면 **읽기·다운로드 코어는 Rust로 완전히 전환 가능**하고,
Python이 런타임에 반드시 필요한 로직은 없다. 남은 것은 언어 문제가 아니라
브라우저 UI에 묶인 OZ PDF 내보내기와 과제 제출이다.

## 실험 결과

`rust/`는 별도 Cargo 크레이트다. Python 0.4.0과 같은 `session.json`을 읽고,
같은 26개 도구에 `get_onboarding_status`를 더한 27개 MCP 도구를 제공한다.

| 항목 | Python 0.4.0 | Rust core |
|---|---:|---:|
| `tools/list`까지 시작 | 약 0.58~0.63초 | **약 0.03초** |
| `tools/list` CPU 시간 | 약 0.62초 | **<0.01초** |
| 최대 RSS | 약 106 MiB | **약 10.5 MiB** |
| 유휴 RSS | 약 106~111 MiB | **약 9.6 MiB** |
| 프로세스 수 | 1 | 1 |
| 릴리스 바이너리 | wheel + Python 환경 | **약 16 MiB 단일 실행 파일** |

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
- MCP stdio JSON-RPC 27개 도구
- `login`, `onboard`, `logout`, `doctor`, `config`, `serve`

## Python이 필수인 부분이 있나

**런타임 코어에는 없다.** 다음은 모두 Rust로 대체됐다.

| 기존 Python | Rust 대체 |
|---|---|
| `pypdf` 페이지 텍스트 | `lopdf` (일반 텍스트 PDF 검증·테스트 완료) |
| FastMCP 서버 | 작은 stdio JSON-RPC 서버 |
| `httpx` 전송 | `reqwest` |
| `asyncio` 병렬 briefing | `tokio` + `buffer_unordered(4)` |
| 세션 수집 | `rookie-cookies` Chrome 프로필 쿠키 |
| 카탈로그·필터 | 순수 Rust 모듈 + 단위 테스트 |

남은 두 기능은 **Python 언어에 묶인 것이 아니라 브라우저 UI에 묶여 있다.**

1. **OZ 계획서 PDF 내보내기**: OZ 뷰어가 `/oz80/server`에서 보고서 데이터를
   받아 브라우저 안에서 PDF를 생성한다. 서버에 바로 받을 PDF URL이 없어
   Rust에서는 브라우저 자동화가 필요하다.
2. **과제 준비·제출**: Canvas 업로드 티켓, CSRF, multipart 업로드, 제출 POST,
   readback 검증을 Rust HTTP로 구현했다. 합성 테스트는 통과했고 실계정 검증만
   남았다.

즉, OZ PDF만 브라우저 도우미로 남기고 나머지는 Rust로 통일할 수 있다.

## OZ PDF 분석 결과

실제 강좌의 수업계획서를 Aside로 열어 읽기 전용으로 관찰했다.

- OZ 뷰어는 `rpt80.cau.ac.kr/oz80/ozViewer.jsp`를 iframe으로 열고,
  보고서 데이터는 `/oz80/server` XHR 4회로 가져온다.
- Save 대화상자에서 `Adobe PDF File(*.pdf)`를 선택하고 OK를 누르면
  브라우저 다운로드 `noname.pdf`가 생성되지만, **추가 네트워크 요청은 없다.**
- 즉 PDF는 서버가 바로 내려주는 파일이 아니라 OZJSViewer가 브라우저 안에서
  보고서 데이터를 렌더링해 만든다. `/oz80/server` 응답에는 PDF URL이 없다.

따라서 OZ PDF를 순수 Rust HTTP만으로 얻는 방법은 사실상 없다. 선택지는:

1. Rust에서 headless Chrome을 제어해 OZ 뷰어를 실행하고 PDF를 받는다
   (`chromiumoxide`, `headless_chrome` 등). CDP를 내부적으로 쓰지만 사용자가
   remote debugging 포트를 관리할 필요는 없다.
2. macOS/Windows WebView(`wry`)에 OZ 뷰어를 띄우고 PDF로 저장한다.
   플랫폼별 동작 차이가 크고 구현 비용이 높다.
3. OZJSViewer를 JS 런타임과 DOM/Canvas로 재현한다. 6.4MB 규모의
   비공개 뷰어 엔진을 유지해야 하므로 권장하지 않는다.
4. OZ PDF만 Python/Playwright 헬퍼로 남긴다. MCP 서버는 Rust 하나로 유지하고,
   필요할 때만 헬퍼를 호출한다.

권장은 1번(가능하면 headless Chrome을 Rust에서 관리) 또는 4번이다. 어느 쪽이든
OZ PDF는 “브라우저 경계”로 남는다. 반면 과제 제출은 Canvas HTTP 업로드로
Rust에 이식했으므로 브라우저가 필요 없다.

## 배포: Rust가 더 단순해질 수 있다

사용자는 Rust, Python, uv, 가상환경을 설치할 필요가 없다. GitHub Releases의
사전 빌드 바이너리와 설치 스크립트만 사용한다.

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.sh | sh
eclass-mcp onboard --client claude
```

릴리스 매트릭스:

- macOS Apple Silicon / Intel
- Linux x86_64 / arm64
- Windows x64

각 아카이브에 SHA-256을 붙인다. **베타 단계에서는 서명·notarization을
필수로 두지 않는다.** macOS Gatekeeper 경고와 Windows SmartScreen 가능성을
릴리스 노트에 솔직히 적고, 버전 고정(`ECLASS_VERSION=rust-v0.1.0`)과 체크섬
검증을 기본으로 한다. Python `uv` 설치는 베타 기간의 fallback으로 유지한다.
사용자가 늘면 macOS Developer ID 서명·notarization을 먼저 추가하고, 그다음
Windows Authenticode를 검토한다. Homebrew tap은 선택 사항이다.

업데이트는 beta에서 자동 업데이터를 두지 않고, 새 `ECLASS_VERSION`으로 설치
스크립트를 다시 실행하는 방식이다. 세션 파일과 다운로드는 설치 디렉터리
밖이라 보존된다. `.github/workflows/rust-release.yml`과 `rust/install.sh`,
`rust/install.ps1`에 스켈레톤을 추가했다. Cargo/Go/Rust 툴체인을 사용자에게
요구하지 않는 경로다.

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
| `OPEN_WEEKLY_LEARNING` | Canvas는 되지만 LearningX 토큰이 없음 |
| `CHECK_ENROLLMENT` | 강좌 조회 실패 또는 0개 |
| `CONFIGURE_CLIENT` | 준비 완료, 클라이언트 설정 단계 |

`session_state`는 `missing`, `expired`, `unavailable`, `authenticated`를
구분한다. 단순히 `authenticated=false`로 뭉개지 않는다. MCP 서버는 세션이
없어도 시작하며, 이 상태에서 `get_onboarding_status`가 `missing`을 반환하고
다른 도구는 로그인 필요 오류를 낸다. 따라서 MCP 클라이언트 안에서도 첫 실행
안내가 가능하다. `get_onboarding_status`는 쿠키 값·Bearer 토큰·세션 파일
내용을 절대 포함하지 않으며, 이를 직렬화 테스트로 고정했다.

LearningX 토큰은 `login`/`onboard`가 `/learningx/main` HTTP 요청 후에도
없으면, 전용 Chrome 프로필로 `learningx/main` 탭을 열고 쿠키 DB를 잠시
폴링한다. 자동화가 실패하면 `OPEN_WEEKLY_LEARNING` 액션으로 남겨 사용자가
**주차학습 (모듈)** 화면을 한 번 열고 `login --no-launch`를 실행하도록
안내한다.

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

## 권장 순서

1. Rust 읽기/다운로드 코어를 실제 계정으로 검증한다.
2. `onboard` JSON 스키마를 `schema_version=1`로 고정하고 Rust에서 완성한다.
   LearningX 탭 자동화 가능 여부를 실제 브라우저에서 확인한다. Python
   쪽에는 새 기능을 얹지 않고, 마이그레이션 기간이 길어질 때만 얇게 추가한다.
3. GitHub Releases + SHA-256 + 설치 스크립트로 **베타 배포**한다. 서명은
   미루고, Python `uv` 경로를 fallback으로 남긴다. 스킬/`AGENTS.md`로
   에이전트 온보딩 흐름을 검증한다.
4. 실제 사용자 피드백을 본 뒤 macOS 서명·notarization, 그다음 Windows
   서명 투자 여부를 결정한다.
5. OZ PDF를 Rust 브라우저 자동화로 포팅하거나 작은 헬퍼로 남긴다. 제출은
   실계정 검증 후 기본 활성화 여부를 결정한다.
6. Rust가 primary가 되면 Python 패키지는 레거시/브라우저 도우미로 축소한다.

두 개의 완전한 구현을 장기간 유지하지 않는다. Rust를 primary로 정하고,
Python은 OZ·제출·PDF 골든 테스트 같은 보조 역할로만 남기는 편이 단순하다.
