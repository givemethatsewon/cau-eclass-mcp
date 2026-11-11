# CAU e-Class MCP

**가볍고 빠른 중앙대학교 e-Class MCP**

로그인만 브라우저에서 하고, 이후 강좌·공지·과제·자료 조회와 다운로드는 HTTP로 처리하는 로컬 MCP 서버입니다. Rust 코어는 단일 바이너리로 약 **0.03초**에 시작하고 **9.6 MiB** RSS로 동작합니다. Python 0.4.0은 OZ 계획서 PDF와 과제 제출까지 포함한 전체 기능 구현입니다.

## 한눈에

| | Rust core | Python 0.4.0 |
|---|---|---|
| 설치 | GitHub Releases 단일 바이너리 | `uv` / Python 환경 |
| 시작 | **약 0.03초** | 약 0.58초 |
| 유휴 메모리 | **약 9.6 MiB** | 약 106~111 MiB |
| 브라우저 | 로그인할 때만 | 로그인 + OZ PDF·제출 |
| 범위 | 읽기·다운로드·PDF 텍스트·제출(HTTP) | OZ PDF·과제 제출 |
| 도구 | 27개 | 26개 |

> Rust 코어는 현재 실험 단계입니다. OZ PDF 내보내기와 실제 과제 제출은 Python/브라우저 경로를 사용합니다.

## 설치

### Rust (가장 가볍고 빠름)

릴리스가 준비되면 GitHub Releases의 사전 빌드 바이너리를 설치합니다. 사용자는 Rust, Python, uv, 가상환경을 설치할 필요가 없습니다.

```sh
curl -fsSL https://raw.githubusercontent.com/givemethatsewon/cau-eclass-mcp/main/rust/install.sh | sh
eclass-mcp onboard --client claude
```

현재 작업 트리에서 직접 빌드하려면:

```sh
cd rust
cargo build --release
./target/release/eclass-mcp-rs onboard --client claude
```

자세한 배포·업데이트·서명 정책은 [rust/README.md](rust/README.md), [rust/RELEASING.md](rust/RELEASING.md)를 참고하세요.

### Python 0.4.0 (전체 기능)

Python 3.12 이상이 필요합니다. [uv](https://docs.astral.sh/uv/getting-started/installation/) 또는 pip으로 설치합니다.

> 0.4.0은 현재 저장소 기준 버전입니다. PyPI 게시 전에는 아래 wheel 또는 소스 설치를 사용하세요.

#### PyPI

```sh
uv tool install --python 3.12 cau-eclass-mcp==0.4.0
eclass-mcp --version
```

[PyPI](https://pypi.org/project/cau-eclass-mcp/) · [GitHub Releases](https://github.com/givemethatsewon/cau-eclass-mcp/releases)

#### pip

```sh
python -m venv .venv
```

macOS/Linux에서는 `source .venv/bin/activate`, Windows PowerShell에서는 `.venv\Scripts\Activate.ps1`로 활성화합니다.

```sh
python -m pip install "cau-eclass-mcp==0.4.0"
python -m eclass_mcp.cli --version
```

이하의 `eclass-mcp`는 `python -m eclass_mcp.cli`로 바꿔 실행해도 됩니다.

#### wheel 또는 소스

```sh
# 전달받은 wheel
uv tool install --python 3.12 --from /absolute/path/cau_eclass_mcp-0.4.0-py3-none-any.whl cau-eclass-mcp

# 저장소 checkout
uv sync --frozen
uv run eclass-mcp --help
```

계획서 OZ PDF 내보내기를 쓸 때만 Playwright가 필요합니다.

```sh
uv tool install --python 3.12 'cau-eclass-mcp[browser]==0.4.0'
```

## 로그인: 한 번만

```sh
# Rust: 로그인 + 세션 검증 + 다음 액션 안내
eclass-mcp onboard --client claude

# Python: 세션 저장
eclass-mcp login
```

Rust와 Python은 같은 `session.json` 형식을 사용합니다. 전용 Chrome/Edge/Chromium 프로필(`~/.cau-eclass/chrome-profile`)이 열리고, e-Class에 직접 로그인하면 세션을 감지해 저장합니다. 기본 대기 시간은 300초입니다.

```sh
eclass-mcp doctor
eclass-mcp config --client claude
```

`doctor`는 세션 파일·로그인·전송 방식을 확인하고 `ok: true/false`를 출력합니다. 이름·학번·쿠키는 출력하지 않습니다. `config`는 설정을 **출력만** 하며 클라이언트 파일을 수정하지 않습니다.

### 로그인 옵션

```sh
# 이미 로그인된 Chrome 프로필의 쿠키만 읽기
eclass-mcp login --no-launch

# 다른 브라우저/프로필
eclass-mcp login --browser-path "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" --browser-id edge
eclass-mcp login --profile-dir ~/.cau-eclass/edge-profile

# 세션 삭제
eclass-mcp logout
```

`ECLASS_BROWSER_PATH`에 실행 파일 경로를 지정해도 됩니다. 로그인 프로필은 전용으로 유지하세요. 평소 Chrome 프로필의 쿠키를 복사하지 않습니다. 로그인 후 브라우저를 닫아도 일반 조회·다운로드는 동작합니다. 세션이 만료되면 다시 `login`을 실행합니다. `login` 출력의 `learningx_token`이 `false`이면 로그인 브라우저에서 **주차학습 (모듈)** 화면을 한 번 연 뒤 `eclass-mcp login --no-launch`를 실행하세요.

### 저장 위치

| OS | 기본 경로 |
|---|---|
| macOS | `~/Library/Application Support/cau-eclass/session.json` |
| Linux | `${XDG_CONFIG_HOME:-~/.config}/cau-eclass/session.json` |
| Windows | `%APPDATA%\cau-eclass\session.json` |

파일은 소유자만 읽을 수 있게 `0600`, 디렉터리는 `0700`으로 저장합니다. `ECLASS_SESSION_FILE`로 위치를 바꿀 수 있습니다.

## 에이전트 온보딩

Rust 코어는 세션이 없어도 MCP 서버가 시작됩니다. 에이전트는 `get_onboarding_status`로 다음 상태를 확인하고 `next_actions[].code`에 따라 안내합니다.

- `RUN_ONBOARD`: 아직 로그인 전
- `SESSION_EXPIRED`: 세션 만료
- `OPEN_WEEKLY_LEARNING`: LearningX 토큰이 없음
- `CHECK_ENROLLMENT`: 강좌 조회 실패 또는 0개
- `CONFIGURE_CLIENT`: 준비 완료

```json
{
  "schema_version": 1,
  "session_state": "missing | expired | unavailable | authenticated",
  "authenticated": false,
  "learningx_token": false,
  "courses_visible": null,
  "ready": false,
  "next_actions": [
    {
      "code": "RUN_ONBOARD",
      "message": "Sign in to e-Class once in the dedicated browser.",
      "command": "eclass-mcp onboard --client claude"
    }
  ]
}
```

에이전트는 비밀번호·OTP·쿠키·세션 파일 내용을 채팅으로 요청하지 않습니다. 사용자가 브라우저에서 직접 로그인하고, 에이전트는 상태를 다시 확인해 다음 단계만 안내합니다. LearningX 토큰은 `onboard`/`login`이 주차학습 탭을 자동으로 열어 확보를 시도하고, 실패하면 수동 안내로 넘어갑니다.

## 왜 가벼운가

- **브라우저는 로그인할 때만** 필요합니다. 이후 Canvas/LearningX JSON 조회와 파일 다운로드는 HTTP로 처리합니다.
- Rust 코어는 **단일 16 MiB 바이너리**, 약 **0.03초 시작**, 약 **9.6 MiB 유휴 RSS**입니다.
- 20초 TTL 캐시와 단일 비행(single-flight)으로 같은 주차·과목 응답을 반복 요청하지 않습니다. `ECLASS_CACHE_TTL=0`으로 끌 수 있습니다.
- `get_briefing`은 과목 공지를 최대 4개씩 병렬로 읽어 한 번에 요약합니다.
- 세션이 만료되면 열려 있는 로그인 브라우저에서 자동으로 다시 읽습니다(`ECLASS_AUTO_REFRESH=1`).
- OZ 계획서 PDF만 브라우저 경로를 사용합니다. 과제 제출은 Canvas HTTP 업로드로 Rust에서도 동작합니다.

## 브라우저 프로필

Rust와 Python 모두 전용 Chrome 프로필(`~/.cau-eclass/chrome-profile`)에 로그인한 뒤 **쿠키 DB를 직접 읽습니다.** remote debugging 포트나 CDP 연결을 사용하지 않습니다.

- `eclass-mcp login`이 Chrome을 열고, 로그인 후 프로필 쿠키를 세션으로 저장합니다.
- Chrome이 쿠키 DB를 잠그고 있으면 로그인 후 Chrome을 닫고 다시 시도하세요.
- `ECLASS_BROWSER=aside`는 Aside 기반 레거시 경로로만 남아 있습니다.
- `ECLASS_CHROME_PROFILE`로 프로필 위치를 바꿀 수 있습니다.

## MCP 클라이언트 연결

```sh
# Claude Desktop 또는 Cursor
eclass-mcp config --client claude

# VS Code
eclass-mcp config --client vscode

# Codex TOML
eclass-mcp config --client codex
```

출력을 사용하는 클라이언트의 MCP 설정에 병합한 뒤 연결을 다시 시작합니다. `config`는 실제 제출을 항상 비활성화한 설정을 출력합니다. 형식별 예시는 [examples/](examples/)에 있습니다.

서버만 직접 실행하려면:

```sh
eclass-mcp serve
```

기본 명령도 `serve`입니다. 웹 포트 없이 stdio로 MCP 요청을 기다립니다.

## 에이전트 스킬 설치 (선택)

스킬은 과목 ID 선택, 마감일 해석, 원본/재생용 영상 구분, 제출 경계를 에이전트에 안내합니다. MCP 설치·로그인을 대신하지 않습니다.

```sh
mkdir -p ~/.agents/skills/cau-eclass
eclass-mcp skill > ~/.agents/skills/cau-eclass/SKILL.md
```

이미 같은 스킬이 있다면 덮어쓰기 전에 내용을 확인하세요. 저장소의 [SKILL.md](skills/cau-eclass/SKILL.md)를 직접 설치할 수도 있습니다.

## 연결 확인과 사용

> eclass의 로그인 상태를 확인하고, 이번 주 마감과 공지를 요약해줘.

`get_session_status`와 `get_briefing`으로 확인할 수 있습니다. Python 0.4.0은 **26개**, Rust 코어는 `get_onboarding_status`를 더한 **27개** 도구를 제공합니다.

- `get_onboarding_status` (Rust): 세션·LearningX 토큰·강좌 접근 상태와 다음 액션을 한 번에 반환합니다.

- `get_briefing(days=14, course_limit=12)`: 수강 과목, 다가오는 todo 마감, 최근 공지를 한 번에 반환합니다. 일부 과목 조회가 실패하면 `complete=false`와 `warnings`를 함께 반환합니다.
- `list_todo`: Canvas 통합 todo를 그대로 반환합니다.
- `list_courses(query="과목명", term="학기 이름 또는 ID")`: 과목을 좁힙니다.

### 강의자료 찾고 읽기

1. `list_courses(query="과목명", term="2026년 2학기")`로 강좌를 찾습니다.
2. `list_materials(course_id=123, week=9, query="Scheduler", content_type="pdf")`로 자료를 찾습니다. 기본 20개이며 `has_more=true`면 `next_offset`으로 다음 결과를 요청합니다.
3. 반환된 `resource_id` 하나를 `download_material` 또는 `read_material`에 전달합니다. Commons item ID와 Canvas file ID를 직접 구분하거나 조립할 필요가 없습니다.
4. `read_material(resource_id="목록에서 받은 값", start_page=1)`은 기본 최대 10페이지·24,000자를 읽습니다. `has_more=true`이면 반환된 `next_page`와 `next_offset`을 각각 `start_page`, `start_offset`으로 전달해 이어 읽습니다.

`list_learning_items`도 `week`, `query`, `content_type` 필터와 간결한 주차·자료 목록을 기본 제공합니다. 기존 LearningX 중첩 메타데이터가 필요할 때만 `detail=true`를 사용합니다.

PDF 응답은 페이지별 본문, 원본 페이지 수, 빈 본문 페이지, 범위 완전성 및 다음 읽기 위치를 포함합니다. `document_complete`는 **이번 호출에서** 문서 전체 텍스트를 읽었는지를 뜻합니다. OCR은 하지 않으므로 텍스트 레이어가 없는 그림·스캔은 별도 시각 확인이 필요합니다.

사용 예시:

- “이번 주 마감 과제와 새 공지를 한 번에 정리해줘.”
- “컴퓨터그래픽스 공지를 이번 학기 시작일부터 오늘까지 조회해줘.”
- “빅데이터 과제와 마감일을 한국 시간으로 정리해줘.”
- “이 과목의 강의계획서를 PDF로 저장하고 본문을 읽어줘.”
- “이 첨부파일을 다운로드해줘.”
- “이 영상 원본을 다운로드해줘. 원본 서버가 실패하면 재생용 MP4로 받아도 돼.”
- “이 파일을 과제로 제출할 수 있는지 준비 검사만 해줘. 제출하지 마.”

직접 도구를 호출하는 클라이언트에서는 다음 인자를 사용합니다. 숫자는 형식 예시이며 본인 계정의 ID로 바꿔야 합니다.

| 도구 | 인자 예시 |
|---|---|
| `get_briefing` | `{"days":14,"course_limit":12}` |
| `list_announcements` | `{"course_id":123,"start_date":"2026-09-01","end_date":"2026-12-31"}` |
| `list_courses` | `{"query":"과목명","term":"2026년 2학기"}` |
| `list_materials` | `{"course_id":123,"week":9,"content_type":"pdf","limit":20}` |
| `download_material` | `{"resource_id":"목록에서 반환된 값"}` |
| `read_material` | `{"resource_id":"목록에서 반환된 값","start_page":1,"end_page":3}` |
| `list_deadlines` | `{"course_id":123}` |
| `download_attachment` | `{"course_id":123,"file_id":456}` |
| `download_syllabus_pdf` | `{"course_id":123,"extract_text":true}` |
| `download_video_original` | `{"course_id":123,"item_id":789,"allow_playback_copy":false}` |
| `prepare_submission` | `{"course_id":123,"assignment_id":456,"files":["/absolute/path/work.pdf"]}` |

도구 응답은 `observed_at`과 `result`로 구성됩니다. 목록의 `complete`·페이지 정보를 확인해 누락된 페이지가 없는지 판단하세요.

## 다운로드

기본 저장 위치는 `~/Downloads/cau-eclass/`입니다. `ECLASS_DOWNLOAD_DIR` 또는 `--download-dir`로 바꿀 수 있습니다. 결과에 로컬 절대 경로, 파일 크기, SHA-256이 반환되며 기존 파일을 덮어쓰지 않습니다.

| 대상 | 사용 방법과 제한 |
|---|---|
| 주차 Commons PDF·Canvas 파일 | `list_materials` → `download_material(resource_id)`. 최대 64 MiB, PDF는 실제 파일·페이지 수 검증. Commons는 서버가 제공하는 네이티브 원본 PDF만 지원 |
| PDF 본문 | `read_material(resource_id)`: 검증된 PDF를 받아 페이지별 텍스트 추출. 같은 프로세스에서는 자료 개정 정보·현재 권한·로컬 해시를 확인해 기존 다운로드 재사용 |
| 기존 Canvas 첨부 | `download_attachment`: 기존 Canvas file ID 방식 유지 |
| 계획서 | `download_syllabus_pdf`: OZ의 Save → Adobe PDF로 전체 페이지 저장. 브라우저와 Playwright 필요 |
| 영상 | `download_video_original`: 접근 가능한 Commons MP4, 최대 1 GiB. `not_open` 영상은 거부 |

계획서는 브라우저 인쇄본이 아닌 **OZ 네이티브 PDF 내보내기**입니다. OCR은 하지 않습니다. `download_syllabus_pdf`는 로그인 브라우저가 열려 있어야 합니다.

영상 원본 다운로드 주소가 HTTP 500을 반환하면 기본 설정에서는 오류를 반환합니다. `allow_playback_copy=true`를 명시하면 플레이어가 제공한 MP4를 대신 내려받고 **`source=commons_playback_mp4`, `original=false`**로 표시합니다. 다운로드는 출석이나 학습 완료로 인정되지 않습니다.

## 전체 도구

| 기능 | 도구 |
|---|---|
| 세션·강좌·요약 | `get_session_status`, `get_onboarding_status`(Rust), `list_courses`, `get_briefing`, `list_todo` |
| 주차학습 | `list_learning_items` |
| 통합 자료 | `list_materials`, `download_material`, `read_material` |
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
| `ECLASS_BROWSER` | 자동 | `http`(권장), `aside` |
| `ECLASS_SESSION_FILE` | OS 설정 경로 | 저장된 세션 파일 위치 |
| `ECLASS_CHROME_PROFILE` | `~/.cau-eclass/chrome-profile` | 로그인·쿠키를 읽을 Chrome 프로필 |
| `ECLASS_AUTO_REFRESH` | `1` | 401 시 열린 로그인 브라우저에서 세션 재수집 |
| `ECLASS_CACHE_TTL` | `20` | 읽기 캐시 초. `0`이면 비활성화 |
| `ECLASS_ASIDE_BIN` | `aside` | Aside CLI 경로 |
| `ECLASS_ASIDE_PROFILE` | Aside 기본 프로필 | 로그인된 Aside 프로필 |
| `ECLASS_DOWNLOAD_DIR` | `~/Downloads/cau-eclass` | 다운로드 폴더 |
| `ECLASS_ENABLE_SUBMISSION` | 비활성화 | 정확히 `1`일 때만 실험적 실제 제출 허용 |

환경 변수는 MCP 클라이언트가 서버를 시작할 때 전달해야 합니다. 변경 후에는 연결을 다시 시작하세요. 개인용 `mcp.json`, 다운로드, 세션 파일은 Git에서 제외합니다.

### 과제 제출

**실제 업로드·제출은 운영 검증되지 않은 실험 기능**입니다. 기본 설정 `ECLASS_ENABLE_SUBMISSION=0`을 유지하세요.

`prepare_submission`은 파일 해시·확장자·과제 잠금·기존 제출을 검사하고 유효기간 10분의 로컬 계획을 만듭니다. 파일을 업로드하지 않습니다. 실제 제출을 명시적으로 요청한 경우에만 `ECLASS_ENABLE_SUBMISSION=1`로 설정하고 MCP 서버를 재시작합니다. 재시작 후 `prepare_submission`을 다시 실행하고, 파일·과제·준비 결과를 검토한 뒤 새 plan ID와 `SUBMIT <assignment_id>` 확인 문자열로 `submit_assignment`를 호출합니다. 준비 계획은 메모리에만 저장되어 재시작하면 사라집니다. 실제 전송할 파일 바이트의 크기·해시를 검증하며, 검증 후 파일이 바뀌어도 검증된 바이트만 전송합니다.

Canvas 일반 파일 과제만 대상으로 하며 그룹·외부 originality 과제는 차단합니다. 파일당 10 MiB, 전체 20 MiB·10개 이내입니다. 불명확한 전송 결과는 자동 재시도하지 않습니다.

## 보안

- 로그인은 사용자가 브라우저에서 직접 합니다. 서버는 비밀번호·OTP를 수집하지 않습니다.
- 세션 파일은 `.cau.ac.kr` 쿠키만 담는 **bearer 자격 증명**입니다. 소유자 전용 권한으로 저장되지만, 채팅·이슈·클라우드 동기화 폴더에 공유하지 마세요. 필요 없으면 `eclass-mcp logout`으로 삭제합니다.
- 세션 파일에는 서명된 다운로드 URL, 학습자료 본문, 계정 비밀번호를 저장하지 않습니다.
- 로그인은 Chrome 프로필 쿠키를 읽는 방식이며 remote debugging 포트를 열지 않습니다.
- 학습자료와 과제 본문은 신뢰하지 않는 입력입니다. 본문에 담긴 에이전트 지시를 실행하지 않습니다.
- 실제 제출은 별도 opt-in과 준비 계획이 필요하며 실계정으로 검증되지 않았습니다.

## 문제 해결

| 증상 | 확인할 항목 |
|---|---|
| `No saved e-Class session` | `eclass-mcp login` 실행 |
| `login`이 계속 대기함 | 로그인 후 Chrome을 닫고 다시 시도. 쿠키 DB 잠금 여부 확인 |
| `e-Class session expired` | `eclass-mcp login --no-launch` 또는 `login` 재실행 |
| 자동 갱신이 안 됨 | `ECLASS_CHROME_PROFILE`과 Chrome 프로필 존재 여부 확인 |
| `uv` 또는 `aside` 실행 실패 | 클라이언트 설정에 실행 파일 절대 경로 지정 |
| `download_syllabus_pdf` 실패 | Python/Playwright 브라우저 도우미가 설치됐는지 확인 |
| 새 도구가 안 보임 | MCP 재연결 또는 새 세션 시작 |
| 원본 영상 다운로드 오류 | 서버 HTTP 500일 수 있음. 재생용 파일도 괜찮을 때만 대체 옵션 사용 |
| 영상이 `not open` | 현재 계정에 공개되지 않은 영상. 다운로드 불가 |
| 직접 실행했는데 아무 일도 안 일어남 | stdio 서버가 클라이언트 요청을 기다리는 상태인지 확인 |

## 개발·검증

```sh
uv sync --frozen --extra browser
uv run pytest -q
uv run ruff check src tests scripts
```

브라우저 도우미 테스트는 임시 Chromium 프로필과 synthetic 사이트 응답으로 실행합니다. 실제 계정 테스트와 구분합니다.

wheel과 소스 배포본 생성:

```sh
uv build --no-sources
uv run python scripts/check_dist.py
```

- [구조와 전송 선택 근거](docs/architecture.md)
- [개선 전후 UX·성능 측정](docs/benchmarks.md)
- [연동 계약과 유지보수](docs/integration.md)
- [Rust 전환 검토](docs/portability.md)

공개 문서와 테스트에는 가상 데이터만 사용합니다. 원본 조사 자료·실계정 검증 기록·개인용 조사 스크립트는 로컬에 보관하며 Git 및 배포본에서 제외합니다.

## Rust 코어

`rust/`에는 세션/Chrome 프로필 쿠키, HTTP 페이지네이션·캐시, 강좌·공지·과제·자료, Commons PDF/영상, Canvas 다운로드, PDF 텍스트와 MCP stdio 서버를 구현한 독립 Rust 크레이트가 있습니다. 같은 `session.json`을 사용하며, 로컬 측정 기준 **약 0.03초 시작 / 9.6 MiB 유휴 RSS / 16 MiB 바이너리**입니다.

```sh
cd rust
cargo run --release -- onboard --client claude
```

사용자는 Rust/Python/uv 없이 GitHub Releases의 사전 빌드 바이너리를 설치하는 것을 목표로 합니다. OZ PDF와 과제 제출은 아직 브라우저 경로가 필요합니다. 자세한 내용은 [rust/README.md](rust/README.md), [rust/RELEASING.md](rust/RELEASING.md), [Rust 전환 검토](docs/portability.md)를 참고하세요.

## 라이선스

[CAU e-Class Noncommercial License 1.0](LICENSE)을 적용합니다. 개인 학습·비상업적 연구를 위한 사용·수정·재배포는 허용하며, 판매·유료 서비스·수익화 제품에 포함·사업 운영을 위한 사용은 금지합니다. 상업적 목적이라면 교육·비영리기관에도 예외를 두지 않습니다.

프로젝트 전용 비상업 라이선스를 쓰는 **소스 공개(source-available)** 프로젝트이며 MIT 또는 OSI 오픈소스 라이선스가 아닙니다. 제3자 의존성은 각자의 라이선스를 유지합니다. 강의자료·대학 상표·사용자 데이터에 대한 권리는 이 라이선스에 포함되지 않습니다.
