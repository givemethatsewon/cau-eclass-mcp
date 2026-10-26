# CAU e-Class MCP

중앙대학교 e-Class의 강좌·공지·과제·마감일을 조회하고, 자료·계획서 PDF·영상을 내려받는 로컬 MCP 서버입니다. FastMCP 4.1.4를 사용하며 MCP 클라이언트와 stdio로 통신합니다.

## 브라우저 지원

**현재 구현은 Aside가 필요합니다.** 로그인된 Aside 탭을 재사용하며, 서버에 아이디·비밀번호를 입력하지 않습니다.

e-Class API 자체가 Aside 전용인 것은 아닙니다. 다만 현재 `AsideTransport`가 Aside의 탭 연결, 브라우저 내 인증 요청, 다운로드 파일 전달 기능을 사용합니다. Chrome·Edge 등 다른 Chromium 브라우저를 사용하려면 이 부분에 CDP 연결과 다운로드 처리를 구현해야 합니다. 현재 CDP 주소나 브라우저 실행 경로를 지정하는 옵션은 없습니다. `ECLASS_ASIDE_BIN`에 Chrome 경로를 넣어 대체할 수 없습니다.

현재 검증 환경은 macOS의 로컬 Aside입니다. 원격 브라우저나 여러 사용자가 공유하는 서버 구성은 지원하지 않습니다.

## 1. 준비

필요한 항목:

- Python 3.12 이상과 `uv`
- 설치·실행 중인 Aside와 `aside` CLI
- stdio MCP를 지원하는 클라이언트

저장소 디렉터리에서 실행합니다.

```sh
uv --version
aside --help
uv sync --frozen
```

Aside에서 다음을 준비합니다.

1. https://eclass3.cau.ac.kr/ 에 로그인합니다.
2. 수강 과목의 **주차학습 (모듈)** 화면을 한 번 열어 LearningX 인증을 준비합니다.
3. 로그인된 e-Class 탭과 Aside를 열어 둡니다.

서버는 로그인 화면을 대신 작성하지 않습니다. 로그인 만료 시 Aside에서 다시 로그인하세요.

## 2. MCP 클라이언트에 연결

### Codex

저장소 디렉터리에서 다음 명령을 실행합니다. `uv`와 `aside`가 PATH에 있어야 합니다.

```sh
codex mcp add eclass \
  --env ECLASS_ENABLE_SUBMISSION=0 \
  --env "ECLASS_ASIDE_BIN=$(command -v aside)" \
  -- "$(command -v uv)" --directory "$PWD" run --frozen eclass-mcp
```

이미 `eclass`가 등록되어 있다면 기존 설정을 사용하거나 필요한 경로만 수정하세요. 등록 또는 코드 업데이트 후에는 MCP 연결을 다시 시작합니다. 기존 채팅에서 도구 목록이 갱신되지 않으면 새 세션을 엽니다.

### 다른 MCP 클라이언트

[mcp.example.json](mcp.example.json)을 참고해 클라이언트의 MCP 설정에 추가합니다. `/absolute/path/to/eclass-mcp`는 실제 저장소 절대 경로로 바꾸세요.

```json
{
  "mcpServers": {
    "eclass": {
      "command": "uv",
      "args": [
        "--directory", "/absolute/path/to/eclass-mcp",
        "run", "--frozen", "eclass-mcp"
      ],
      "env": {
        "ECLASS_ENABLE_SUBMISSION": "0"
      }
    }
  }
}
```

GUI 클라이언트가 실행 파일을 찾지 못하면 `command -v uv`의 결과를 `command`에 넣고, `command -v aside`의 결과를 `env.ECLASS_ASIDE_BIN`에 넣으세요. 이 JSON은 파일에 저장하는 것만으로 자동 등록되지 않으며, 사용하는 클라이언트의 설정에 적용해야 합니다.

직접 시작할 수도 있습니다.

```sh
uv run --frozen eclass-mcp
```

이 명령은 웹페이지를 열거나 포트를 제공하지 않습니다. stdio MCP 요청을 기다리는 서버이므로, 일반 사용은 위 클라이언트 연결 방식을 이용하세요.

## 3. 연결 확인과 사용

연결한 에이전트에게 다음 순서로 요청합니다.

> eclass의 로그인 상태를 확인하고, 내 강좌 목록을 보여줘.

`get_session_status`와 `list_courses`로 확인할 수 있습니다. 현재 제공하는 도구는 **21개**입니다. 과목 ID는 강좌 목록에서, 과제·파일·영상 ID는 해당 목록이나 상세 응답에서 선택합니다.

사용 예시:

- “컴퓨터그래픽스 공지를 이번 학기 시작일부터 오늘까지 조회해줘.”
- “빅데이터 과제와 마감일을 한국 시간으로 정리해줘.”
- “이 과목의 강의계획서를 PDF로 저장하고 본문을 읽어줘.”
- “이 첨부파일을 다운로드해줘.”
- “이 영상 원본을 다운로드해줘. 원본 서버가 실패하면 재생용 MP4로 받아도 돼.”
- “이 파일을 과제로 제출할 수 있는지 준비 검사만 해줘. 제출하지 마.”

직접 도구를 호출하는 클라이언트에서는 다음 인자를 사용합니다. 숫자는 형식 예시이며 본인 계정의 ID로 바꿔야 합니다.

| 도구 | 인자 예시 |
|---|---|
| `list_announcements` | `{"course_id":123,"start_date":"2026-09-01","end_date":"2026-12-31"}` |
| `list_deadlines` | `{"course_id":123}` |
| `download_attachment` | `{"course_id":123,"file_id":456}` |
| `download_syllabus_pdf` | `{"course_id":123,"extract_text":true}` |
| `download_video_original` | `{"course_id":123,"item_id":789,"allow_playback_copy":false}` |
| `prepare_submission` | `{"course_id":123,"assignment_id":456,"files":["/absolute/path/work.pdf"]}` |

도구 응답은 `observed_at`과 `result`로 구성됩니다. 목록의 `complete`·페이지 정보를 확인해 누락된 페이지가 없는지 판단하세요.

## 다운로드

기본 저장 위치는 서버 작업 디렉터리의 `downloads/`입니다. 위 설정대로 실행하면 저장소 안에 생성됩니다. 결과에 로컬 절대 경로, 파일 크기, SHA-256이 반환되며 기존 파일을 덮어쓰지 않습니다.

| 대상 | 사용 방법과 제한 |
|---|---|
| Canvas 첨부 | `download_attachment`: Canvas file ID 사용, 최대 64 MiB. OCS 일반 주차자료와 LearningX 게시판 전용 첨부는 미지원 |
| 계획서 | `download_syllabus_pdf`: OZ의 Save → Adobe PDF로 전체 페이지 저장. `extract_text=true`가 기본값이며 페이지별 본문도 반환. `false`는 PDF만 저장. 64 MiB 초과 결과는 거부 |
| 영상 | `download_video_original`: 접근 가능한 Commons MP4 대상, 최대 1 GiB. `not_open` 영상은 거부 |

계획서는 브라우저 인쇄본이 아닌 **OZ 네이티브 PDF 내보내기**입니다. OCR은 하지 않으며, 텍스트가 추출되지 않은 페이지가 있으면 `text_complete=false`를 반환합니다. `get_syllabus`는 별도로 HTML 본문과 뷰어 링크를 조회하는 도구입니다.

실제 시험한 영상 원본 다운로드 주소는 학교 서버에서 HTTP 500을 반환했습니다. 기본 설정에서는 오류를 반환합니다. `allow_playback_copy=true`를 명시하면 플레이어의 정적 메타데이터에 제공된 MP4를 대신 내려받습니다. 이 경우 **`source=commons_playback_mp4`, `original=false`**로 반환합니다. 원본 다운로드 성공 시에는 `source=commons_original`, `original=true`입니다.

영상 다운로드 과정에서 플레이어 스크립트·재생·진도 API를 실행하지 않습니다. 다운로드는 출석이나 학습 완료로 인정되지 않습니다.

## 전체 도구

| 기능 | 도구 |
|---|---|
| 세션·강좌 | `get_session_status`, `list_courses` |
| 주차학습 | `list_learning_items` |
| 공지 | `list_announcements`, `get_announcement` |
| 과제·마감 | `list_assignments`, `get_assignment`, `list_deadlines`, `get_submission` |
| 계획서 | `get_syllabus`, `download_syllabus_pdf` |
| 게시판 | `list_boards`, `list_board_posts`, `get_board_post` |
| 그룹 | `list_my_groups`, `get_group_activity` |
| 다운로드 | `download_attachment`, `get_video_download_info`, `download_video_original` |
| 제출 준비·제출 | `prepare_submission`, `submit_assignment` |

## 설정

| 환경 변수 | 기본값 | 용도 |
|---|---|---|
| `ECLASS_ASIDE_BIN` | `aside` | Aside CLI 경로 |
| `ECLASS_ASIDE_PROFILE` | Aside 기본 프로필 | 로그인된 Aside 프로필 선택 |
| `ECLASS_DOWNLOAD_DIR` | 현재 디렉터리의 `downloads` | 다운로드 폴더. 절대 경로 권장 |
| `ECLASS_ENABLE_SUBMISSION` | 비활성화 | 정확히 `1`일 때만 실험적 실제 제출 허용 |

환경 변수는 MCP 클라이언트가 서버를 시작할 때 전달해야 합니다. 변경 후에는 연결을 다시 시작하세요. `.env` 파일을 자동으로 읽는 기능은 없습니다. 개인용 `mcp.json`, 다운로드, 세션·인증 파일은 Git에서 제외합니다.

### 과제 제출

**실제 업로드·제출은 운영 검증되지 않은 실험 기능**입니다. 기본 설정 `ECLASS_ENABLE_SUBMISSION=0`을 유지하세요.

`prepare_submission`은 파일 해시·확장자·과제 잠금·기존 제출을 검사하고 유효기간 10분의 로컬 계획을 만듭니다. 파일을 업로드하지 않습니다. 실제 제출을 명시적으로 요청한 경우에만 설정을 `1`로 바꾸고, 준비 결과의 plan ID와 `SUBMIT <assignment_id>` 확인 문자열로 `submit_assignment`를 호출합니다.

Canvas 일반 파일 과제만 대상으로 하며 그룹·외부 originality 과제는 차단합니다. 파일당 10 MiB, 전체 20 MiB·10개 이내입니다. 별도 redirect 기반 업로드 완료 절차는 미지원입니다. 불명확한 전송 결과는 자동 재시도하지 않습니다.

## 문제 해결

| 증상 | 확인할 항목 |
|---|---|
| `uv` 또는 `aside` 실행 실패 | 클라이언트 설정에 실행 파일 절대 경로 지정 |
| 로그인된 탭을 찾지 못함 | Aside에서 e-Class 로그인 후 탭 유지. 프로필 설정 확인 |
| 로그인·LearningX 요청 실패 | 다시 로그인하고 과목의 주차학습 화면을 연 뒤 재시도 |
| 새 다운로드 도구가 안 보임 | MCP 재연결 또는 새 세션 시작 |
| 원본 영상 다운로드 오류 | 서버 HTTP 500일 수 있음. 재생용 파일도 괜찮을 때만 대체 옵션 사용 |
| 영상이 `not open` | 현재 계정에 공개되지 않은 영상. 다운로드 불가 |
| 계획서 저장 실패 | 해당 과목에 OZ 뷰어와 PDF 저장 메뉴가 있는지 확인 |
| 직접 실행했는데 아무 일도 안 일어남 | stdio 서버가 클라이언트 요청을 기다리는 상태인지 확인 |

## 개발·검증

```sh
uv sync --frozen
uv run pytest -q
uv run ruff check src tests scripts
```

실계정 검증 스크립트는 로그인된 Aside가 필요하며 조회·로컬 다운로드·제출 준비/차단 검사를 수행합니다. 실제 과제 제출은 하지 않습니다. **과목·자료 ID가 특정 검증 계정 기준으로 작성되어 있으므로 다른 계정에서는 먼저 수정하세요.**

```sh
uv run python scripts/live_smoke.py
uv run python scripts/live_followup.py
uv run python scripts/live_documents.py
```

검증 기록: 자동 테스트 15개 통과, MCP 도구 21개 등록, 계획서 PDF 4페이지·한글 본문 추출, 재생용 MP4 약 95 MB 저장 확인. 원본 영상 endpoint 성공과 실제 과제 제출 성공은 검증되지 않았습니다.

- [실제 검증 결과와 제한](docs/verification.md)
- [구조와 FastMCP 선택 근거](docs/architecture.md)
- [인증·기능 분석 기록](analysis/authenticated-findings.md)

`analysis/`는 구현 전 조사 기록입니다. 현재 지원 범위는 이 README와 검증 기록을 기준으로 확인하세요. 분석 원본 JS·HTML은 Git에 포함하지 않아 새 checkout에는 없을 수 있습니다.
