# CAU e-Class MCP

**비공식 중앙대학교 e-Class 도구 · 비상업적 사용만 허용 · 상업적 사용 금지**

중앙대학교 e-Class의 강좌·공지·과제·마감일을 조회하고, 자료·계획서 PDF·영상을 내려받는 로컬 MCP 서버입니다. FastMCP 4.1.4를 사용하며 MCP 클라이언트와 stdio로 통신합니다.

## 설치

Python 3.12 이상이 필요합니다. [uv](https://docs.astral.sh/uv/getting-started/installation/) 또는 pip으로 **0.3.0**을 설치할 수 있습니다.

### PyPI로 설치

```sh
# Chrome·Edge 등 로컬 Chromium CDP 사용
uv tool install --python 3.12 'cau-eclass-mcp[cdp]==0.3.0'

# 설치 확인
eclass-mcp --version
```

Aside만 사용한다면 `uv tool install --python 3.12 cau-eclass-mcp==0.3.0`으로 설치합니다.
[PyPI](https://pypi.org/project/cau-eclass-mcp/) · [GitHub Releases](https://github.com/givemethatsewon/cau-eclass-mcp/releases)

### uv 없이 pip으로 설치

가상환경을 만든 뒤 설치합니다. `python` 명령이 없다면 macOS/Linux에서는 `python3`, Windows에서는 `py`를 사용하세요.

```sh
python -m venv .venv
```

macOS/Linux에서는 `source .venv/bin/activate`, Windows PowerShell에서는 `.venv\Scripts\Activate.ps1`로 활성화합니다.

```sh
# Chrome·Edge 등 로컬 Chromium CDP 사용
python -m pip install "cau-eclass-mcp[cdp]==0.3.0"

# 설치 확인과 MCP 설정 생성
python -m eclass_mcp.cli --version
python -m eclass_mcp.cli --browser cdp config --client claude
```

Aside만 사용한다면 `python -m pip install "cau-eclass-mcp==0.3.0"`로 설치하고 `--browser aside`로 설정을 생성합니다. 이하의 `eclass-mcp` 명령은 `python -m eclass_mcp.cli`로 바꿔 실행해도 됩니다. 생성된 MCP 설정은 가상환경의 Python 절대 경로를 사용하므로 해당 가상환경을 삭제하거나 옮기지 마세요.

### 전달받은 wheel로 설치

저장소 checkout 없이 wheel 파일을 설치할 수 있습니다. 아래 경로는 전달받은 파일 위치로 바꿉니다.

```sh
# Chrome·Edge 등 로컬 Chromium CDP 사용
uv tool install --python 3.12 --from /absolute/path/cau_eclass_mcp-0.3.0-py3-none-any.whl --with 'playwright>=1.60,<2' cau-eclass-mcp

# 설치 확인
eclass-mcp --version
```

Aside만 사용하는 경우 설치 명령의 `--with 'playwright>=1.60,<2'`를 생략합니다. `eclass-mcp`가 PATH에 없다면 `uv tool update-shell`을 실행한 뒤 터미널을 다시 엽니다.

### 소스에서 실행

```sh
uv sync --frozen --extra cdp
uv run eclass-mcp --help
```

이하의 `eclass-mcp` 명령은 소스 실행 시 `uv run eclass-mcp`로 바꾸면 됩니다. 설치된 Chrome·Edge에 연결만 하므로 `playwright install`로 별도 브라우저를 받을 필요는 없습니다.

## 브라우저 선택과 로그인

| 방식 | 필요한 것 | 선택 |
|---|---|---|
| Chromium CDP | 전용 프로필로 remote debugging을 켠 Chrome·Edge 등 | `--browser cdp` |
| Aside | 실행 중인 로컬 Aside, `aside` CLI | `--browser aside` (기본값) |

### Chromium CDP

[운영체제별 CDP 시작 가이드](docs/cdp.md)에 따라 브라우저를 실행합니다. macOS Chrome 예시:

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cau-eclass/chrome-profile" \
  https://eclass3.cau.ac.kr/
```

이 전용 프로필에서 직접 로그인하고 **주차학습 (모듈)** 화면을 한 번 연 뒤 탭을 유지합니다. 최초 로그인 이후에는 같은 프로필을 재사용합니다. 평소 프로필의 쿠키를 복사하지 않습니다.

```sh
eclass-mcp --browser cdp --cdp-endpoint http://127.0.0.1:9222 doctor
```

### Aside

Aside에서 https://eclass3.cau.ac.kr/ 에 로그인하고 **주차학습 (모듈)** 화면을 한 번 엽니다. 탭과 Aside를 유지합니다.

```sh
aside --help
eclass-mcp --browser aside doctor
```

`doctor`는 프로필 조회로 연결·로그인을 검사하고 `ok: true/false`를 출력합니다. 이름·학번·쿠키는 출력하지 않습니다. 실패 시 종료 코드는 1입니다. 로그인 만료 시 선택한 브라우저에서 다시 로그인하세요.

## MCP 클라이언트 연결

설치 진단이 통과한 브라우저 설정으로 클라이언트 설정을 생성합니다. 이 명령은 설정을 **출력만** 하며 파일을 덮어쓰거나 클라이언트를 변경하지 않습니다.

```sh
# Claude Desktop 또는 Cursor 형식
eclass-mcp --browser cdp --cdp-endpoint http://127.0.0.1:9222 config --client claude

# VS Code 형식
eclass-mcp --browser cdp config --client vscode

# Codex TOML 형식
eclass-mcp --browser cdp config --client codex
```

Aside는 `--browser aside`로 바꾸고 CDP 옵션을 생략합니다. 필요하면 `--download-dir /absolute/path/downloads`도 함께 전달하세요. 공통 옵션은 `doctor`, `config`, `serve` 등의 명령 **앞에** 놓습니다.

출력을 사용하는 클라이언트의 MCP 설정에 병합한 뒤 연결을 다시 시작합니다. 설정의 Python 경로는 설치 환경의 절대 경로이므로 패키지 설치 위치를 바꾸면 다시 생성하세요. `config`는 실제 제출을 항상 비활성화한 설정을 출력합니다. 형식별 예시는 [examples/](examples/)에 있습니다.

서버만 직접 실행하려면:

```sh
eclass-mcp --browser cdp serve
```

기본 명령도 `serve`입니다. 웹 포트를 제공하지 않고 stdio로 MCP 요청을 기다립니다. 사용자 브라우저는 서버 종료 후에도 유지됩니다.

## 에이전트 스킬 설치 (선택)

스킬은 과목 ID 선택, 마감일 해석, 원본/재생용 영상 구분, 제출 경계를 에이전트에 안내합니다. MCP 설치·브라우저 로그인을 대신하지 않습니다.

```sh
mkdir -p ~/.agents/skills/cau-eclass
eclass-mcp skill > ~/.agents/skills/cau-eclass/SKILL.md
```

이미 같은 스킬이 있다면 덮어쓰기 전에 내용을 확인하세요. 클라이언트가 다른 스킬 디렉터리를 사용하면 경로를 바꿉니다. 저장소의 [SKILL.md](skills/cau-eclass/SKILL.md)를 직접 설치할 수도 있습니다.

## 연결 확인과 사용

연결한 에이전트에게 다음 순서로 요청합니다.

> eclass의 로그인 상태를 확인하고, 내 강좌 목록을 보여줘.

`get_session_status`와 `list_courses`로 확인할 수 있습니다. 현재 **24개** 도구를 제공합니다. 과목은 `list_courses(query="과목명", term="학기 이름 또는 ID")`로 좁힙니다. 자료는 `list_materials`가 반환한 `resource_id`를 그대로 사용합니다. 과제·영상 ID도 해당 목록이나 상세 응답에서 선택합니다. 실행 중인 MCP는 재연결해야 새 도구와 인자 목록을 읽습니다.

### 강의자료 찾고 읽기

1. `list_courses(query="과목명", term="2026년 2학기")`로 강좌를 찾습니다. 검색은 한글 정규화·대소문자·공백 차이를 처리합니다.
2. `list_materials(course_id=123, week=9, query="Scheduler", content_type="pdf")`로 자료를 찾습니다. 기본 20개이며 `has_more=true`면 `next_offset`으로 다음 결과를 요청합니다.
3. 반환된 `resource_id` 하나를 `download_material` 또는 `read_material`에 전달합니다. Commons item ID와 Canvas file ID를 직접 구분하거나 조립할 필요가 없습니다.
4. `read_material(resource_id="목록에서 받은 값", start_page=1)`은 기본 최대 10페이지·24,000자를 읽습니다. `has_more=true`이면 반환된 `next_page`와 `next_offset`을 각각 `start_page`, `start_offset`으로 전달해 이어 읽습니다. 한 페이지 중간에서 끊겼을 때도 같은 페이지의 남은 본문부터 이어집니다.

`list_learning_items`도 `week`, `query`, `content_type` 필터와 간결한 주차·자료 목록을 기본 제공합니다. 기존 LearningX 중첩 메타데이터가 필요할 때만 `detail=true`를 사용합니다.

PDF 응답은 페이지별 본문, 원본 페이지 수, 빈 본문 페이지, 범위 완전성 및 다음 읽기 위치를 포함합니다. `document_complete`는 **이번 호출에서** 문서 전체 텍스트를 읽었는지를 뜻합니다. 여러 호출로 읽었다면 각 응답의 SHA-256이 같은지 확인하고 범위를 모아 완전성을 확인해야 합니다. OCR은 하지 않으므로 텍스트 레이어가 없는 그림·스캔은 별도 시각 확인이 필요합니다. 자료 전체의 요약본을 원문 대신 반환하지 않습니다.

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

기본 저장 위치는 사용자 홈의 `~/Downloads/cau-eclass/`입니다. `ECLASS_DOWNLOAD_DIR` 또는 `--download-dir`로 바꿀 수 있습니다. 결과에 로컬 절대 경로, 파일 크기, SHA-256이 반환되며 기존 파일을 덮어쓰지 않습니다.

| 대상 | 사용 방법과 제한 |
|---|---|
| 주차 Commons PDF·Canvas 파일 | `list_materials` → `download_material(resource_id)`. 최대 64 MiB, PDF는 실제 파일·페이지 수 검증. Commons는 서버가 제공하는 네이티브 원본 PDF만 지원하며, 변환된 슬라이드를 원본 PDF라고 표시하지 않음 |
| PDF 본문 | `read_material(resource_id)`: 검증된 PDF를 받아 페이지별 텍스트 추출. 같은 프로세스에서는 자료 개정 정보·현재 권한·로컬 해시를 확인해 기존 다운로드 재사용 |
| 기존 Canvas 첨부 | `download_attachment`: 기존 Canvas file ID 방식도 유지. LearningX 게시판 전용 첨부는 아직 미지원 |
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
| `ECLASS_BROWSER` | `aside` | `aside` 또는 `cdp` 선택 |
| `ECLASS_CDP_ENDPOINT` | `http://127.0.0.1:9222` | 로컬 Chromium CDP 주소 |
| `ECLASS_ASIDE_BIN` | `aside` | Aside CLI 경로 |
| `ECLASS_ASIDE_PROFILE` | Aside 기본 프로필 | 로그인된 Aside 프로필 선택 |
| `ECLASS_DOWNLOAD_DIR` | `~/Downloads/cau-eclass` | 다운로드 폴더. 절대 경로 권장 |
| `ECLASS_ENABLE_SUBMISSION` | 비활성화 | 정확히 `1`일 때만 실험적 실제 제출 허용 |

환경 변수는 MCP 클라이언트가 서버를 시작할 때 전달해야 합니다. 변경 후에는 연결을 다시 시작하세요. `.env` 파일을 자동으로 읽는 기능은 없습니다. 개인용 `mcp.json`, 다운로드, 세션·인증 파일은 Git에서 제외합니다.

### 과제 제출

**실제 업로드·제출은 운영 검증되지 않은 실험 기능**입니다. 기본 설정 `ECLASS_ENABLE_SUBMISSION=0`을 유지하세요.

`prepare_submission`은 파일 해시·확장자·과제 잠금·기존 제출을 검사하고 유효기간 10분의 로컬 계획을 만듭니다. 파일을 업로드하지 않습니다. 실제 제출을 명시적으로 요청한 경우에만 `ECLASS_ENABLE_SUBMISSION=1`로 설정하고 MCP 서버를 재시작합니다. 재시작 후 `prepare_submission`을 다시 실행하고, 파일·과제·준비 결과를 검토한 뒤 새 plan ID와 `SUBMIT <assignment_id>` 확인 문자열로 `submit_assignment`를 호출합니다. 준비 계획은 메모리에만 저장되어 재시작하면 사라집니다. 실제 전송할 파일 바이트의 크기·해시를 검증하며, 검증 후 파일이 바뀌어도 검증된 바이트만 전송합니다.

Canvas 일반 파일 과제만 대상으로 하며 그룹·외부 originality 과제는 차단합니다. 파일당 10 MiB, 전체 20 MiB·10개 이내입니다. 별도 redirect 기반 업로드 완료 절차는 미지원입니다. 불명확한 전송 결과는 자동 재시도하지 않습니다.

## 문제 해결

| 증상 | 확인할 항목 |
|---|---|
| `uv` 또는 `aside` 실행 실패 | 클라이언트 설정에 실행 파일 절대 경로 지정 |
| 로그인된 탭을 찾지 못함 | 선택한 브라우저에서 e-Class 로그인 후 탭 유지. 프로필 설정 확인 |
| CDP 연결 실패 | 디버깅 포트와 전용 프로필 확인, `doctor` 재실행 |
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

CDP 연결 테스트는 임시 Chromium 프로필과 synthetic 사이트 응답으로 실행합니다. 실제 계정 테스트와 구분하며, macOS에서 로컬 검증하고 Linux/Windows 빌드는 CI에 구성했습니다. CI 구성 자체를 해당 운영체제 검증 완료로 간주하지 않습니다. 테스트 실행 방법은 [개발 안내](CONTRIBUTING.md)를 참고하세요.

wheel과 소스 배포본 생성:

```sh
uv build --no-sources
uv run python scripts/check_dist.py
```

[배포 절차와 참고 설계](docs/distribution.md)에 패키지 설치, 산출물 제외 규칙, 공개 게시 전 남은 설정을 정리했습니다. 실제 과제 제출은 검증하지 않았고, 원본 영상 서버의 HTTP 500 제한은 남아 있습니다.

- [구조와 FastMCP 선택 근거](docs/architecture.md)
- [연동 계약과 유지보수](docs/integration.md)

공개 문서와 테스트에는 가상 데이터만 사용합니다. 원본 조사 자료·실계정 검증 기록·개인용 조사 스크립트는 로컬에 보관하며 Git 및 배포본에서 제외합니다.


## 라이선스

[CAU e-Class Noncommercial License 1.0](LICENSE)을 적용합니다. 개인 학습·비상업적 연구를 위한 사용·수정·재배포는 허용하며, 판매·유료 서비스·수익화 제품에 포함·사업 운영을 위한 사용은 금지합니다. 상업적 목적이라면 교육·비영리기관에도 예외를 두지 않습니다. 정확한 허용 범위와 조건은 LICENSE 원문을 따릅니다.

프로젝트 전용 비상업 라이선스를 쓰는 **소스 공개(source-available)** 프로젝트이며 MIT 또는 OSI 오픈소스 라이선스가 아닙니다. 제3자 의존성은 각자의 라이선스를 유지합니다. 강의자료·대학 상표·사용자 데이터에 대한 권리는 이 라이선스에 포함되지 않습니다.
