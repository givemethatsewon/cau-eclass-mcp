# CAU e-Class MCP

**가볍고 빠른 중앙대학교 e-Class MCP**

로그인만 브라우저에서 하고, 강좌·공지·과제·자료·다운로드·제출은 HTTP로 처리합니다. MCP 서버는 Rust 단일 바이너리이고, Python은 OZ 계획서 PDF를 만들 때만 쓰는 작은 헬퍼입니다. **FastMCP 서버는 더 이상 사용하지 않습니다.**

## 구조

| 구성 | 역할 |
|---|---|
| Rust `eclass-mcp` | MCP stdio 서버, 27개 도구, HTTP 조회·다운로드·PDF 텍스트·과제 제출 |
| Python `eclass-oz-export` (선택) | OZ 뷰어의 Save → Adobe PDF를 Playwright로 실행하는 PDF 전용 헬퍼 |
| `session.json` | 두 구성이 공유하는 `.cau.ac.kr` 쿠키 저장소. 소유자 전용 `0600` |

OZ PDF는 서버에서 PDF URL을 받는 방식이 아니라 OZJSViewer가 브라우저 안에서 PDF를 생성합니다. 그래서 이 한 기능만 브라우저 헬퍼로 남깁니다. 과제 제출은 Canvas HTTP 업로드로 Rust에 이식했고, OZ PDF는 Rust MCP가 필요할 때만 헬퍼를 호출합니다.

## Rust MCP 설치

릴리스가 준비되면 GitHub Releases의 사전 빌드 바이너리를 사용합니다. Rust, Python, uv, 가상환경을 설치할 필요가 없습니다.

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

## 로그인: 한 번만

```sh
eclass-mcp onboard --client claude
# 또는
eclass-mcp login
```

전용 Chrome/Edge/Chromium 프로필(`~/.cau-eclass/chrome-profile`)이 열리고, e-Class에 직접 로그인하면 쿠키 DB를 읽어 세션을 저장합니다. remote debugging 포트나 CDP 연결은 사용하지 않습니다.

```sh
eclass-mcp doctor
eclass-mcp config --client claude
```

`doctor`는 세션 파일·로그인·전송 방식을 확인하고 `ok: true/false`만 출력합니다. 이름·학번·쿠키는 출력하지 않습니다. `config`는 설정을 **출력만** 하며 클라이언트 파일을 수정하지 않습니다.

### 로그인 옵션

```sh
# 이미 로그인된 Chrome 프로필의 쿠키만 읽기
eclass-mcp login --no-launch

# 다른 브라우저/프로필
eclass-mcp login --browser-path "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
eclass-mcp login --profile-dir ~/.cau-eclass/edge-profile

# 세션 삭제
eclass-mcp logout
```

`ECLASS_BROWSER_PATH`에 실행 파일 경로를 지정해도 됩니다. 로그인 프로필은 전용으로 유지하세요. 로그인 후 브라우저를 닫아도 일반 조회·다운로드는 동작합니다. 세션이 만료되면 다시 `login`을 실행합니다.

### 저장 위치

| OS | 기본 경로 |
|---|---|
| macOS | `~/Library/Application Support/cau-eclass/session.json` |
| Linux | `${XDG_CONFIG_HOME:-~/.config}/cau-eclass/session.json` |
| Windows | `%APPDATA%\cau-eclass\session.json` |

파일은 소유자만 읽을 수 있게 `0600`, 디렉터리는 `0700`으로 저장합니다. `ECLASS_SESSION_FILE`로 위치를 바꿀 수 있습니다.

## OZ 계획서 PDF 헬퍼 (선택)

OZ PDF를 쓰지 않으면 Python을 설치할 필요가 없습니다. 쓰려면 Playwright가 포함된 헬퍼를 설치합니다.

```sh
# 배포 후
uv tool install --python 3.12 'cau-eclass-mcp[oz]==0.5.0'

# 아직 PyPI 배포 전이면 로컬 wheel
uv tool install --python 3.12 --from ./dist/cau_eclass_mcp-0.5.0-py3-none-any.whl 'cau-eclass-mcp[oz]'

# 저장소 checkout에서 직접 쓸 때
uv sync --extra oz

export ECLASS_OZ_HELPER="$(command -v eclass-oz-export)"
```

헬퍼는 시스템 Chrome/Edge/Chromium을 우선 사용하고, 없으면 Playwright Chromium을 사용합니다. Playwright 브라우저가 없으면 한 번 설치합니다.

```sh
uv run playwright install chromium
```

Rust MCP 설정의 `env`에 `ECLASS_OZ_HELPER` 절대 경로를 넣으면 클라이언트가 다른 `PATH`로 서버를 띄워도 동작합니다.

```json
{
  "mcpServers": {
    "eclass": {
      "command": "/absolute/path/to/eclass-mcp",
      "args": ["serve"],
      "env": {
        "ECLASS_SESSION_FILE": "/absolute/path/session.json",
        "ECLASS_OZ_HELPER": "/absolute/path/eclass-oz-export",
        "ECLASS_ENABLE_SUBMISSION": "0"
      }
    }
  }
}
```

## MCP 클라이언트 연결

```sh
eclass-mcp config --client claude
eclass-mcp config --client vscode
eclass-mcp config --client codex
```

출력을 사용하는 클라이언트의 MCP 설정에 병합한 뒤 연결을 다시 시작합니다. `config`는 실제 제출을 항상 비활성화한 설정을 출력합니다. 서버만 직접 실행하려면 `eclass-mcp serve`를 사용합니다. stdio로 MCP 요청을 기다리며 웹 포트를 열지 않습니다.

## 도구

Rust MCP는 27개 도구를 제공합니다. 기존 26개 도구에 `get_onboarding_status`를 더한 구성입니다.

| 기능 | 도구 |
|---|---|
| 세션·강좌·요약 | `get_session_status`, `get_onboarding_status`, `list_courses`, `get_briefing`, `list_todo` |
| 주차학습 | `list_learning_items` |
| 통합 자료 | `list_materials`, `download_material`, `read_material` |
| 공지 | `list_announcements`, `get_announcement` |
| 과제·마감 | `list_assignments`, `get_assignment`, `list_deadlines`, `get_submission` |
| 계획서 | `get_syllabus`, `download_syllabus_pdf` |
| 게시판 | `list_boards`, `list_board_posts`, `get_board_post` |
| 그룹 | `list_my_groups`, `get_group_activity` |
| 다운로드 | `download_attachment`, `get_video_download_info`, `download_video_original` |
| 제출 | `prepare_submission`, `submit_assignment` |

`download_syllabus_pdf`는 OZ 헬퍼가 설치된 경우에만 동작합니다. 나머지 26개 도구는 Rust만으로 동작합니다.

### 온보딩 상태

Rust MCP는 세션이 없어도 시작됩니다. 에이전트는 `get_onboarding_status`의 `next_actions[].code`를 보고 안내합니다.

- `RUN_ONBOARD`: 아직 로그인 전
- `SESSION_EXPIRED`: 세션 만료
- `OPEN_WEEKLY_LEARNING`: LearningX 토큰이 없음
- `CHECK_ENROLLMENT`: 강좌 조회 실패 또는 0개
- `CONFIGURE_CLIENT`: 준비 완료

비밀번호·OTP·쿠키·세션 파일 내용을 채팅으로 요청하지 않습니다.

## 과제 제출

과제 제출은 Canvas HTTP 업로드로 Rust에 구현되어 있습니다. 기본값은 **비활성화**입니다.

1. `prepare_submission`은 파일 해시·확장자·과제 잠금·기존 제출을 검사하고 10분짜리 로컬 계획을 만듭니다. 업로드하지 않습니다.
2. 실제 제출을 명시적으로 요청한 경우에만 `ECLASS_ENABLE_SUBMISSION=1`로 서버를 재시작합니다.
3. 새 plan ID와 `SUBMIT <assignment_id>` 확인 문자열로 `submit_assignment`를 호출합니다.
4. 업로드 티켓 → multipart 업로드(쿠키·인증 헤더 없이) → 제출 POST → readback 검증 순서로 진행합니다.

합성 wiremock 테스트는 통과했지만 실계정 제출은 아직 검증하지 않았습니다. 불명확한 결과는 자동 재시도하지 않습니다. 그룹·외부 originality 과제와 파일당 10 MiB/전체 20 MiB 제한을 넘는 제출은 차단합니다.

## 왜 가벼운가

- 일반 조회·다운로드·제출은 **로그인할 때만** 브라우저가 필요합니다.
- Rust MCP는 **17 MiB 단일 바이너리**, 약 **0.04초 시작**, 약 **9.6 MiB 유휴 RSS**입니다.
- 20초 TTL 캐시와 단일 비행(single-flight)으로 같은 과목·주차 응답을 반복 요청하지 않습니다.
- `get_briefing`은 과목 공지를 최대 4개씩 병렬로 읽습니다.
- 세션이 만료되면 저장된 Chrome 프로필에서 쿠키를 다시 읽습니다(`ECLASS_AUTO_REFRESH=1`).
- FastMCP와 상시 Python 런타임을 제거했습니다. OZ PDF를 쓸 때만 Python 헬퍼가 실행됩니다.

## 보안

- 로그인은 사용자가 브라우저에서 직접 합니다. 서버는 비밀번호·OTP를 수집하지 않습니다.
- 세션 파일은 `.cau.ac.kr` 쿠키만 담는 **bearer 자격 증명**입니다. 채팅·이슈·클라우드 동기화 폴더에 공유하지 마세요.
- 세션 파일에는 서명된 다운로드 URL, 학습자료 본문, 계정 비밀번호를 저장하지 않습니다.
- 학습자료와 과제 본문은 신뢰하지 않는 입력입니다. 본문에 담긴 에이전트 지시를 실행하지 않습니다.
- 실제 제출은 별도 opt-in과 준비 계획이 필요합니다.

## 설정

| 환경 변수 | 기본값 | 용도 |
|---|---|---|
| `ECLASS_SESSION_FILE` | OS 설정 경로 | 저장된 세션 파일 위치 |
| `ECLASS_CHROME_PROFILE` | `~/.cau-eclass/chrome-profile` | 로그인·쿠키를 읽을 Chrome 프로필 |
| `ECLASS_AUTO_REFRESH` | `1` | 401 시 Chrome 프로필에서 세션 재수집 |
| `ECLASS_CACHE_TTL` | `20` | 읽기 캐시 초. `0`이면 비활성화 |
| `ECLASS_DOWNLOAD_DIR` | `~/Downloads/cau-eclass` | 다운로드 폴더 |
| `ECLASS_ENABLE_SUBMISSION` | 비활성화 | 정확히 `1`일 때만 실제 제출 허용 |
| `ECLASS_OZ_HELPER` | `eclass-oz-export` | OZ PDF 헬퍼 실행 파일 경로 |
| `ECLASS_OZ_BROWSER` | 시스템 Chrome 자동 탐색 | OZ 헬퍼가 사용할 브라우저 실행 파일 |

환경 변수는 MCP 클라이언트가 서버를 시작할 때 전달해야 합니다. 변경 후에는 연결을 다시 시작하세요.

## 개발·검증

```sh
# Rust
cd rust
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release

# Python OZ helper
uv sync --extra oz
uv run pytest -q
uv run ruff check src tests
```

현재 테스트는 Rust 30개(단위 20 + PDF 2 + 서비스 8)와 Python 8개입니다. 실제 계정 로그인·OZ PDF·과제 제출은 별도 검증 대상이며, 공개 테스트에는 가상 데이터만 사용합니다.

- [Rust 빌드·배포](rust/README.md)
- [에이전트 온보딩 스키마](rust/ONBOARDING.md)
- [Rust 포팅 노트](rust/PORTING.md)
- [OZ PDF 분석과 브라우저 경계](docs/portability.md)
- [구조와 전송 선택 근거](docs/architecture.md)

## 라이선스

비상업적 사용만 허용합니다. 자세한 내용은 [LICENSE](LICENSE)와 [NOTICE](NOTICE)를 확인하세요.
