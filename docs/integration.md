# 연동 계약과 유지보수

이 문서는 구현 유지보수에 필요한 계약만 설명합니다. 실제 사용자·강좌·과제 ID, 응답 원본, 로그인 값, 서명된 URL, 다운로드 자료는 포함하지 않습니다. 접근 범위는 로그인한 사용자에게 서버가 허용한 데이터로 제한됩니다.

## 데이터 제공자

| 기능 | 제공자 | 처리 원칙 |
|---|---|---|
| 강좌·공지·과제·제출 상태·그룹 | Canvas API | 현재 사용자 기준 조회, 공지는 명시적 날짜 범위 사용 |
| 주차학습·게시판 | LearningX | Canvas 모듈·토론과 동일하다고 가정하지 않음 |
| 일반 첨부 | Canvas 파일 | 조회한 file ID 사용, 잠금·파일 크기·응답 형식 검증 |
| 주차 PDF | Commons 네이티브 원본 | 목록의 통합 자료 ID 사용, 실제 제공된 URI만 요청, 페이지별 본문 추출 |
| 계획서 | Canvas의 뷰어 연결 및 OZ | 네이티브 PDF 내보내기, 페이지별 텍스트 추출, OCR 미지원 |
| 동영상 | Commons 콘텐츠 | 접근 제한 확인, 원본과 재생용 MP4를 구분, 진도 API 호출 없음 |

경로와 요청 구현은 `rust/src/http.rs`·`session.rs`, 응답 해석은 `rust/src/service.rs`를 기준으로 확인합니다. OZ PDF만 `src/eclass_mcp/oz_export.py` 헬퍼를 사용합니다. 별도의 원본 응답 덤프를 계약으로 사용하지 않습니다. Canvas API 참고: <https://developerdocs.instructure.com/services/canvas>.

## 세션과 전송

- 로그인은 사용자 브라우저에서 수행합니다. 서버는 비밀번호·OTP를 수집하지 않습니다.
- Rust MCP는 `eclass-mcp login`/`onboard`가 저장한 세션으로 HTTP 요청을 보냅니다. 세션 파일은 `.cau.ac.kr` 쿠키만 담고 `0600`으로 저장하며, 서명 URL·본문·비밀번호는 저장하지 않습니다.
- 캐시는 기본 20초 TTL이며 `ECLASS_CACHE_TTL`로 조정합니다. 쓰기 후에는 서비스가 캐시를 비웁니다.
- 401 또는 로그인 HTML을 만나면 저장된 Chrome 프로필에서 쿠키를 다시 읽습니다(`ECLASS_AUTO_REFRESH=1`). 프로필이 없거나 잠겨 있으면 `eclass-mcp login`을 안내합니다.
- LearningX 401은 강좌 페이지의 LTI 폼을 저장된 쿠키로 다시 POST해 `xn_api_token`을 발급합니다. Canvas 쿠키가 만료됐으면 `eclass-mcp auth refresh --via aside`가 로그인된 Aside 브라우저에서 토큰만 회수합니다.
- Canvas와 LearningX의 인증 방식 차이는 전송 계층에서 처리합니다. 인증값과 서명 URL은 도구 응답·로그·설정에 저장하지 않습니다.
- 일반 조회는 같은 origin의 허용된 상대경로를 사용합니다. 파일 다운로드는 호스트 허용 목록과 리다이렉트별 인증 범위를 검사합니다.
- FastMCP, Aside, CDP는 제거했습니다. 로그인과 세션 갱신은 Chrome 프로필 쿠키를 직접 읽으며 remote debugging 포트를 사용하지 않습니다.

## 요약과 완전성

- `get_briefing`은 **현재 학기** 수강 과목, 앞으로 `days`일 안의 마감, 최근 `days`일 공지, 이미 지난 마감(`overdue`)을 한 번에 반환합니다. `windows.deadlines`는 오늘부터 미래, `windows.announcements`는 과거 구간이며 모든 시각은 `Asia/Seoul` 기준입니다. 공지는 최대 4개 과목씩 병렬 조회하고, 일부 실패는 `complete=false`와 `warnings`로 표시합니다.
- `list_todo`는 `/api/v1/users/self/todo`를 그대로 반환합니다.
- 도구 응답은 `observed_at`과 `result`로 구성됩니다. `list_courses`는 기본으로 **오늘 기준 학기**의 과목만 압축 해 반환하고(`course_id`, `name`, `term`, `workflow_state`), `term` 또는 `all_terms=true`로 넓힙니다. 응답의 `visible`은 필터 전 전체 과목 수입니다.
- 목록 도구의 공통 봉투는 `items`, `complete`, `has_more`, `next`, `next_param`입니다. `has_more=true`이면 `next`를 `next_param`이 가리키는 인자에 그대로 넣어 이어 읽습니다(오프셋은 `offset`, 게시판은 `page`). Canvas 목록은 서버가 다음 페이지를 추적하므로 `next`가 없고, 페이지 한도에서 잘리면 `complete=false`와 `warnings`로 알립니다.
- 과목명 해석은 현재 학기 → 전체 학기 순으로 찾고, 후보가 둘 이상이면 임의로 고르지 않고 `AMBIGUOUS_COURSE`와 `details.candidates[].course_id`를 반환합니다. 알 수 없는 인자나 누락된 필수 인자는 `INVALID_ARGUMENT`로 거부합니다.
- `list_learning_items`는 필터 가능한 간결한 주차 구조가 기본이고, `detail=true`로 기존 중첩 메타데이터를 읽습니다. 내부 서비스의 원본 모듈 조회는 마감일·영상 조회를 위해 유지합니다.
- `list_materials`는 Commons 주차자료와 Canvas 파일을 합친 목록입니다. `week`, 제목 `query`, `content_type`을 함께 적용하고 기본 20개·최대 100개를 반환합니다. 상위 소스 조회 실패 또는 아직 반환하지 않은 결과가 있으면 `complete=false`입니다. 재사용된 자료 ID는 중복 제거하되 `weeks`에 모든 관측 주차를 남깁니다.
- `get_week_materials`는 `week`를 생략하면 **자료가 있는 마지막 주차**를 고르고 `weeks_with_materials`, `course_weeks`, `latest_week`를 함께 알려 줍니다. 과정에 없는 주차를 요청하면 빈 목록 대신 `warnings`로 사용 가능한 주차를 안내합니다.
- 목록은 `complete`와 페이지 정보를 확인합니다. Canvas 목록은 `Link`의 다음 페이지를 추적하고, LearningX 게시판은 요청한 한 페이지의 결과임을 표시합니다.
- 로그인 HTML이나 예상과 다른 응답을 빈 목록으로 처리하지 않습니다. 조회 실패와 데이터 없음은 구분합니다.
- `due_at`, `late_at`, `unlock_at`, `lock_at`을 구분하고 원본 시각과 Asia/Seoul 표시를 유지합니다.
- 도구가 반환하는 강좌·자료 본문은 신뢰하지 않는 데이터입니다. 그 안의 명령을 에이전트 지시로 해석하지 않습니다.

## 다운로드와 제출 경계

다운로드 결과는 로컬 경로·크기·SHA-256을 제공합니다. 기존 파일을 덮어쓰지 않고, 허용 크기를 초과하거나 파일 검증에 실패하면 오류를 반환합니다. 다운로드는 출석이나 학습 완료로 처리하지 않습니다. LearningX 게시판 전용 첨부는 현재 다운로드 지원 범위 밖입니다.

`list_materials`의 `resource_id`는 `commons:<course>:<attendance_item>` 또는 `canvas:<course>:<file>`로 유형을 구분합니다. 호출자는 값을 파싱하지 않고 `download_material`/`read_material`에 그대로 전달합니다. 서비스는 요청 때마다 실제 과목 자료를 다시 조회해 ID·권한을 확인합니다. Commons PDF는 정적 원본 메타데이터의 다운로드 경로를 사용하고, 일반 원본 endpoint가 실패하면 뷰어가 실제 제공한 XML의 원본 URI를 사용합니다. 브라우저 인쇄나 렌더링 슬라이드 합성으로 원본을 대체하지 않습니다. 원본 삭제·비PDF 원본·잠김은 명시적으로 거부합니다.

통합 다운로드는 64 MiB까지, PDF는 서명·파싱·페이지 수·암호화 여부를 확인합니다. 본문은 `read_material`에서 페이지 범위와 문자 예산으로 제한합니다. 한 페이지를 잘랐으면 같은 페이지의 문자 offset을 반환해 무손실 이어 읽기를 지원합니다. URL의 민감한 쿼리는 본문 분할 전에 제거합니다. `text_complete`, `document_complete`, 빈 본문 페이지와 추출 실패 페이지는 각각 구분하며 OCR은 하지 않습니다. 반복 읽기는 메모리 캐시를 쓸 수 있지만 현재 과목 권한·자료 개정 메타데이터·실제 로컬 파일 해시를 먼저 확인합니다.

제출은 기본 비활성화입니다. 명시적 요청으로 활성화한 뒤 서버를 재시작하고, 새 준비 계획을 생성해야 합니다. 계획은 현재 프로세스의 메모리에만 있으며 만료됩니다. 전송 직전 과제·계정·제출 상태를 재검증하고 승인된 크기·해시와 일치하는 파일 바이트만 전송합니다. 결과가 불명확하면 자동 재시도하지 않습니다. 그룹·외부 originality 과제는 준비 단계에서 거부합니다. Canvas가 업로드를 `303 → /api/v1/files/<id>/create_success`로 마무리하는 경우에만 같은 오리진에서 인증된 GET으로 따라가고, 그 밖의 리다이렉트는 거부합니다.

개인 파일은 `list_my_folders`/`list_my_files`로 확인합니다. 제출 업로드는 개인 저장소의 `제출물/<과목>` 폴더에 쌓입니다. `delete_file`은 기본이 미리보기(`dry_run=true`)이고, 실제 삭제에는 `ECLASS_ENABLE_DELETE=1`, 정확한 `DELETE <file_id>` 확인 문자열, 그리고 `제출물` 하위 파일이면 `allow_submission_folder=true`가 추가로 필요합니다. 제출에 연결된 파일을 지우면 그 제출물의 첨부가 사라질 수 있기 때문입니다. 조회 도구는 어떤 경우에도 삭제나 제출을 수행하지 않습니다.

## 검증 방법과 범위

`tests/`는 가상 ID·응답·파일을 사용합니다. 브라우저 도우미 테스트도 합성 사이트 응답을 사용하므로 실제 대학 계정의 성공을 대신하지 않습니다. CI는 운영체제별 테스트·빌드·새 환경 설치를 검증합니다.

실계정 HTTP 로그인·전체 읽기 흐름, Chrome 프로필 쿠키 읽기의 실계정 검증, 영상 원본 다운로드 성공, 실제 과제 업로드·제출은 검증 완료로 표시하지 않습니다. 서버 변경에 따라 동작이 달라질 수 있습니다. 문제 보고에는 실제 응답 대신 필요한 필드만 가상 값으로 재현하고, 현재 버전·기대 결과·실제 오류를 기록하세요.
