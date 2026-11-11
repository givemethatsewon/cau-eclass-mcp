# Go/Rust 이전 검토

2026-11-12, 0.4.0 기준. 결론부터 말하면 **Go 이전은 충분히 가능**하다. 0.4.0에서 읽기 경로를 HTTP로 분리했기 때문에, 브라우저 의존성은 로그인 세션 수집과 OZ PDF·제출 두 곳만 남는다.

## 설계 배경

읽기 경로는 화면 로딩 대신 HTTP JSON을 사용하고, 로그인 세션과 짧은 TTL 캐시를 재사용한다. 과목 요약은 병렬 조회로 처리한다. 이 구조는 언어에 독립적이어서 Go/Rust로 옮길 때도 같은 데이터 흐름을 유지할 수 있다.

## 현재 구조의 이식성

0.4.0의 읽기 경로는 다음 네 덩어리다.

1. `session.py`: CDP `Storage.getCookies` → `.cau.ac.kr` 쿠키 필터 → `0600` JSON.
2. `http_transport.py`: cookie jar, `xn_api_token` Bearer, 같은 origin GET, Link 페이지네이션, 허용 호스트 다운로드.
3. `catalog.py` / `documents.py`: 자료 ID·주차·필터, PDF 검증·페이지 텍스트.
4. `server.py`: 26개 MCP 도구 스키마.

1~2는 Go 표준 라이브러리로 거의 1:1 이식된다. 3의 카탈로그 로직은 순수 함수라 포팅하기 쉽고, PDF 텍스트만 라이브러리 선택이 필요하다. 4는 MCP Go SDK를 쓰거나 stdio JSON-RPC를 직접 구현하면 된다.

## Go를 고른다면

| 영역 | 선택 |
|---|---|
| HTTP | `net/http`, `net/http/cookiejar`, `encoding/json` |
| CDP 쿠키 | `nhooyr.io/websocket` 또는 `github.com/gorilla/websocket` + `Storage.getCookies` |
| MCP | `github.com/modelcontextprotocol/go-sdk` (안정성 확인 후) 또는 150줄 내외 stdio JSON-RPC |
| HTML | `golang.org/x/net/html` |
| PDF 텍스트 | `github.com/ledongthuc/pdf` (기본 텍스트), 필요 시 외부 `pdftotext` fallback |
| 배포 | `CGO_ENABLED=0 go build`, GoReleaser로 macOS/Linux/Windows 바이너리 |
| 설정 | 기존 `session.json` 형식 유지 |

예상 규모:

- 세션·HTTP·페이지네이션: 400~600줄
- 카탈로그·필터: 250~350줄
- MCP 도구 스키마·핸들러: 500~700줄
- PDF 텍스트: 100~200줄
- 테스트: 400~600줄

읽기 도구 전체를 2~4일, OZ/제출을 chromedp로 포팅하면 2~3일 더 걸리는 규모다. 단일 바이너리 기준 런타임 RSS는 Python+httpx보다 작아질 가능성이 높지만, 실제 수치는 빌드 후 측정해야 한다.

## Rust를 고른다면

`reqwest` + `tokio` + `serde` + `tower`로 같은 구조를 만들 수 있다. 바이너리 크기와 메모리는 Go보다 더 작아질 수 있지만, 비동기·에러 처리·HTML/PDF 라이브러리 선택과 컴파일 시간이 늘어난다. 이 프로젝트의 병목은 네트워크와 학교 API이므로, 특별한 성능 요구가 없다면 Go가 유지보수 비용 면에서 낫다.

## 이식하지 않는 편이 나은 부분

- **OZ 계획서 PDF**: OZ 뷰어의 Save 대화상자와 다운로드 이벤트를 조작해야 한다. `chromedp`로 가능하지만 브라우저 런타임이 다시 필요하다. Go 본체에서는 "Python/브라우저 도우미 필요"로 두거나, 별도 `eclass-browser` 바이너리로 분리하는 편이 낫다.
- **과제 제출**: Canvas 업로드 API는 HTTP로 가능하지만, CSRF·업로드 티켓·재제출·불명확한 결과 처리까지 실계정 검증이 필요하다. 현재도 실험 기능이므로 Go 1차 범위에서 제외하는 것을 권한다.
- **PDF 텍스트 품질**: Python `pypdf`와 Go 라이브러리의 추출 결과가 다를 수 있다. 중요 문서는 골든 테스트로 비교해야 한다.

## 권장 순서

1. Python 0.4.0을 기준 구현으로 유지하고, 실제 계정으로 HTTP 읽기·다운로드를 검증한다.
2. Go로 `login`, `doctor`, `get_session_status`, `list_courses`, `get_briefing`, `list_todo`만 먼저 만든다.
3. `list_materials`/`read_material`/다운로드를 포팅하고 Python과 결과 해시를 비교한다.
4. OZ·제출은 브라우저 도우미로 남길지, chromedp로 합칠지 결정한다.
5. Go가 읽기 도구에서 충분히 검증되면 Python 패키지는 레거시/브라우저 도우미로 축소한다.

두 개의 완전한 구현을 장기간 유지하는 것은 권하지 않는다. Go를 primary로 삼는다면, Python은 OZ·제출·PDF 검증 같은 브라우저/문서 보조 역할로만 남기는 편이 단순하다.
