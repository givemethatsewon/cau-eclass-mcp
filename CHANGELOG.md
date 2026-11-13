# Changelog

## 0.6.0 — unreleased

- **입력 검증**: MCP 서버가 `inputSchema`를 직접 강제합니다. 오타 난 인자(`max_pages` 오타 등)와 누락된 필수 인자, 잘못된 타입은 `INVALID_ARGUMENT`로 거부하고 허용 인자 목록을 함께 알려 줍니다. 그동안 클라이언트가 조용히 버리던 값 때문에 필터가 무시되던 문제를 없앴습니다.
- **과목명 모호성**: `course=` 이름이 여러 과목과 일치하면 첫 결과를 고르지 않고 `AMBIGUOUS_COURSE`와 `details.candidates[].course_id`를 반환합니다. 현재 학기를 먼저 찾고, 그다음 전체 학기를 봅니다.
- **현재 학기 기본값**: `list_courses`가 기본으로 오늘 기준 학기만 압축 반환(과목당 약 950B → 120B)하고 `all_terms=true`로 넓힙니다. `get_briefing`도 같은 기준을 씁니다.
- **목록 봉투 통일**: 모든 목록이 `items`, `complete`, `has_more`, `next`, `next_param`을 사용합니다. `data`(게시판)와 `next_cursor`(재사용 불가 URL)를 제거했습니다.
- **브리핑 시간창**: `windows.deadlines`(앞으로 `days`일)와 `windows.announcements`(최근 `days`일)를 분리하고, 이미 지난 마감은 `overdue`로, 창 밖 마감은 `deadlines_beyond_window`로 집계합니다.
- **주차 기본값**: `get_week_materials`가 자료가 있는 마지막 주차를 고르고 `weeks_with_materials`, `course_weeks`를 반환합니다. 과정에 없는 주차는 빈 목록 대신 사용 가능한 주차를 안내합니다.
- **계획서 대안 안내**: `get_syllabus`가 `available`과 `suggested_tool`(`download_syllabus_pdf`)을 반환해 OZ 뷰어 문서에서 막다른 길로 끝나지 않습니다.
- **PDF 커서**: `max_pages`(1–20)를 실제로 적용하고, `next_offset`을 `has_more=true`일 때만 `next_page`와 쌍으로 반환합니다.
- **다운로드 정리**: `eclass-mcp downloads [--keep N] [--dry-run]`으로 내려받은 파일 수·용량을 확인하고 오래된 파일을 정리합니다.
- **과제 상세 시각**: `get_assignment`가 원본 필드에 `due_at_seoul`, `lock_at_seoul` 등을 더하고, `get_submission`이 `submitted_at_seoul`을 반환합니다. 목록과 상세의 시간대 표현이 같아졌습니다.
- **제출 경로 검증**: `prepare_submission` → `submit_assignment` 전 구간을 mock Canvas 테스트로 고정하고, 실제 계정의 열린 과제에서 업로드·제출·재조회까지 확인했습니다.
- **업로드 마무리**: Canvas 로컬 스토리지가 업로드 후 `303 → /api/v1/files/{id}/create_success`로 보내는 흐름을 처리합니다. 같은 오리진의 해당 경로일 때만 인증된 GET으로 따라가고, 그 밖의 리다이렉트는 계속 거부합니다.
- **쿼리 검증 분리**: `local_path`가 경로만 검사하고 쿼리는 보존합니다. `verifier`의 `%2F` 때문에 업로드 마무리가 거부되던 문제를 고쳤습니다.
- **단계별 제출 오류**: 업로드 실패 시 `request upload slot` / `upload file` / `record submission` 중 어느 단계에서 무엇이 실패했는지 반환합니다.
- **게시판 내성**: 상세 조회가 실패한 게시글 하나가 게시판 전체 자료 목록을 실패시키지 않습니다.
- **LearningX 토큰 자가 복구**: 저장된 Canvas 쿠키로 강좌의 LTI 폼을 다시 POST해 `xn_api_token`을 발급. 브라우저·재로그인 없이 `auth refresh --via http`로 동작하고, 401 응답에서 자동 재시도.
- **Aside 토큰 브리지**: `auth refresh --via aside`가 사용자가 이미 로그인한 Aside 브라우저에서 토큰만 회수. 전용 Chrome 프로필 로그인을 강요하지 않음.
- **정직한 상태 진단**: `doctor`가 `canvas_ok`, `learningx_ok`, `learningx_token_age_secs`, `error_code`, `next_action`을 분리해 반환. Canvas 쿠키는 살아 있는데 LearningX만 죽은 상태를 구분.
- **자료 통합**: `list_materials`가 Commons 영상, Canvas 파일, LearningX 게시판(강의자료실) 첨부를 합침. `board:<course>:<board>:<post>:<attachment>` resource ID로 다운로드·PDF 읽기 지원. `get_week_materials` 추가.
- **현재 주차**: `list_learning_items`/`list_materials`가 `current_week`와 `weeks`를 반환하고, 게시판 제목의 `N주차`를 인식.
- **응답 위생**: `list_assignments` 기본 compact(`detail=true`로 원본), 게시판 시각 KST 병기, `get_briefing` 현재 학기 우선 정렬·이전 학기 제외, OZ 계획서 URL의 `&amp;` 복원.
- **오류 코드**: `CANVAS_SESSION_EXPIRED`, `LEARNINGX_TOKEN_MISSING`, `LEARNINGX_UNAUTHORIZED`, `PERMISSION_DENIED`, `COURSE_NOT_FOUND` 등 안정 코드와 `next_action`을 MCP 오류에 포함.
- `auth refresh --via auto|http|aside`, 과목명으로 `course_id`를 대신하는 `course` 인자 추가.

## 0.5.0 — 2026-11-12

- **Rust MCP가 기본 서버**: FastMCP와 Python 전체 MCP 서버를 제거하고 Rust stdio JSON-RPC 서버 27개 도구로 통일.
- **과제 제출을 Rust HTTP로 이식**: CSRF, Canvas 업로드 티켓, multipart 업로드(쿠키 미전송), 제출 POST, readback 검증. 기본 비활성화.
- **Python은 OZ PDF 헬퍼로 축소**: `eclass-oz-export`가 `session.json`을 읽고 Playwright로 OZ Save → PDF를 실행. Rust MCP가 필요할 때 subprocess로 호출하고 경로·PDF 헤더·크기를 재검증.
- CDP, Aside, FastMCP 의존성과 레거시 Python HTTP 서비스 제거.
- Python 의존성은 `pypdf`와 선택적 `oz` extra(`playwright`)만 유지.

## 0.4.0 — 2026-11-12

- **HTTP 우선 전송**: `eclass-mcp login`으로 전용 브라우저 세션을 한 번 저장하고, 이후 Canvas/LearningX 조회·다운로드는 브라우저 없이 `httpx`로 처리. `Storage.getCookies`로 HttpOnly 쿠키를 수집한다.
- `login`, `logout` 명령과 `ECLASS_SESSION_FILE` 추가. 세션 파일은 `.cau.ac.kr` 쿠키만 담고 `0600`으로 저장.
- 20초 TTL 캐시와 single-flight로 반복 GET을 줄이고, 401/로그인 HTML 시 열린 로그인 브라우저에서 세션 자동 갱신.
- `get_briefing`, `list_todo` 추가. 과목·todo 마감·최근 공지를 한 번에 반환하며 공지는 최대 4개 과목씩 병렬 조회.
- HTTP 모드에서 Commons PDF·영상 원본/재생용 MP4 다운로드 지원. OZ 계획서 PDF와 실제 제출은 기존 브라우저 경로로 위임.
- `ECLASS_BROWSER` 기본값을 자동 선택(http → aside → http)으로 변경. 기존 `aside`/`cdp` 설정은 그대로 동작.
- 개선 전후 측정: 서버 트리 RSS 231→약 111 MiB, 6과목 요약 요청 13→8회, 합성 지연 0.679→0.212초. 자세한 방법과 한계는 `docs/benchmarks.md`.
- 문서·스킬에 HTTP 세션, 캐시, 보안 경계, Go/Rust 이전 검토 추가.

## 0.3.0 — 2026-10-27

- Commons 네이티브 원본 PDF 다운로드와 페이지별 본문 읽기. 원본 endpoint 실패 시 뷰어가 제공한 원본 URI 사용.
- `list_materials`, `download_material`, `read_material` 통합 자료 도구와 provider-qualified `resource_id` 추가.
- 과목명·학기 필터, 주차·제목·자료유형 필터, 기본 간결 응답과 페이지네이션 추가. `detail=true`로 기존 주차 메타데이터 조회 유지.
- 긴 PDF 본문의 페이지·문자 offset 이어 읽기, 빈 본문/미완료 표시, 자료 권한·개정·파일 해시 재검증 후 다운로드 재사용.
- Aside/CDP 합성 계약, 실제 Aside 강의자료 다운로드·본문 읽기 검증. 공개 테스트에는 가상 자료만 포함.

## 0.2.1 — 2026-10-26

- 제출 재검증 중 파일 변경을 거부하고, 승인된 크기·해시와 일치하는 바이트만 전송. 브라우저 어댑터가 파일을 다시 읽지 않도록 수정.
- 설치 확인 명령과 PATH 복구 안내 수정.
- 제출 활성화·서버 재시작 후 새 준비 계획을 생성하도록 안내 수정.
- 실제 과제 업로드·제출 없이 가상 transport로 회귀 검증.

## 0.2.0 — 2026-10-26

- Aside 및 로컬 Chromium CDP 어댑터 선택.
- 브라우저 API 스크립트 공유, CDP 연결 해제 시 사용자 브라우저 유지.
- `serve`, `doctor`, `config`, `skill`, `--version` CLI.
- Claude/Cursor, VS Code, Codex 설정 출력과 에이전트 스킬 포함.
- OZ 계획서 PDF·본문 추출, 원본 영상 요청과 명시적 재생용 MP4 대체 옵션.
- 기본 다운로드 위치: `~/Downloads/cau-eclass` (`ECLASS_DOWNLOAD_DIR`로 변경 가능).
- wheel/sdist 빌드, 민감한 로컬 산출물 제외, CI와 synthetic CDP 테스트.
- 실제 제출은 기본 비활성화. PyPI 및 GitHub 최초 공개 배포. MCP Registry는 미등록.
- 상업적 사용을 금지하는 프로젝트 전용 비상업 라이선스 적용.
- Windows·macOS·Ubuntu 설치 검증, Windows 파일 삭제 및 UTF-8 출력 수정.

## 0.1.0

- Aside 기반 Canvas/LearningX 조회·Canvas 첨부 다운로드·과제 제출 준비.
