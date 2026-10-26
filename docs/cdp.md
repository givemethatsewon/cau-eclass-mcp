# Chromium CDP 연결

CDP 모드는 로컬 Chrome·Edge 등 Chromium 계열 브라우저에 연결합니다. `cau-eclass-mcp[cdp]` 설치가 필요합니다. Playwright의 연결 API를 사용하며 브라우저를 자동 설치하거나 시작하지 않습니다. Firefox·Safari는 지원하지 않습니다.

## 1. 전용 프로필로 브라우저 시작

Chrome 136부터 기본 사용자 데이터 디렉터리에서는 remote debugging 플래그가 적용되지 않습니다. 아래 전용 디렉터리를 사용하세요. 평소 프로필을 복사하거나 쿠키를 가져올 필요는 없습니다. 처음 한 번 해당 브라우저에 직접 로그인하고 이후 같은 디렉터리를 재사용합니다.

macOS Chrome:

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cau-eclass/chrome-profile" \
  https://eclass3.cau.ac.kr/
```

Linux Chrome:

```sh
google-chrome \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cau-eclass/chrome-profile" \
  https://eclass3.cau.ac.kr/
```

Windows PowerShell (Chrome 기본 설치 경로):

```powershell
& "$env:ProgramFiles\Google\Chrome\Application\chrome.exe" `
  --remote-debugging-address=127.0.0.1 `
  --remote-debugging-port=9222 `
  --user-data-dir="$env:LOCALAPPDATA\cau-eclass\chrome-profile" `
  https://eclass3.cau.ac.kr/
```

Edge는 실행 파일 경로를 Edge로 바꾸고 별도 프로필 디렉터리를 사용합니다. 이미 같은 전용 프로필로 브라우저가 실행 중이면 그 인스턴스를 재사용하세요. 포트는 로컬에만 열어 두며 외부에 노출하지 마세요. 서버도 loopback 주소만 허용합니다.

## 2. 로그인 후 연결 확인

열린 브라우저에서 로그인하고 수강 과목의 **주차학습 (모듈)** 화면을 한 번 엽니다. 브라우저와 탭을 유지한 채 실행합니다.

```sh
eclass-mcp --browser cdp --cdp-endpoint http://127.0.0.1:9222 doctor
eclass-mcp --browser cdp --cdp-endpoint http://127.0.0.1:9222 config --client claude
```

`doctor`가 `ok: true`를 반환하면 설정 출력물을 MCP 클라이언트에 적용하세요. 환경 변수로는 `ECLASS_BROWSER=cdp`, `ECLASS_CDP_ENDPOINT=http://127.0.0.1:9222`를 사용합니다.

서버 종료는 연결만 해제합니다. 기존 탭과 브라우저는 닫지 않습니다. OZ 내보내기용으로 서버가 만든 임시 탭은 작업 후 닫습니다. 여러 계정의 e-Class 컨텍스트가 동시에 발견되면 자동으로 하나를 고르지 않고 오류를 반환합니다.

## 인증과 제한

Canvas/LearningX API는 연결된 페이지에서 요청합니다. 다운로드는 목적지에 맞는 브라우저 쿠키만 프로세스 메모리에서 사용하고, 각 리다이렉트의 목적지와 쿠키 범위를 다시 확인합니다. 쿠키 파일·스토리지 덤프를 만들지 않으며, 인증값과 서명 URL은 도구 응답·일반 오류에 노출하지 않습니다.

지원 여부를 검증한 사이트 흐름을 재현한 synthetic 테스트와 실제 Chromium CDP 연결 테스트를 구분합니다. CDP 어댑터의 실제 대학 계정 검증 여부는 배포 기록을 확인하세요. 학교 UI나 브라우저 버전 변경으로 일부 기능이 달라질 수 있습니다.

참고: [Chrome remote debugging 변경](https://developer.chrome.com/blog/remote-debugging-port), [Playwright connect_over_cdp](https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp).
