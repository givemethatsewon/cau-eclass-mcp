# Chromium CDP와 로그인 세션

0.4.0의 기본 실행은 브라우저를 계속 띄워 두지 않습니다. `login`이 전용 Chromium 프로필에서 쿠키를 한 번 읽어 세션 파일로 저장하고, 이후 조회·다운로드는 HTTP로 처리합니다.

## 로그인

```sh
eclass-mcp login
```

Chrome/Edge/Chromium이 `~/.cau-eclass/chrome-profile` 프로필과 `127.0.0.1:9222` 디버깅 포트로 열립니다. 열린 창에서 e-Class에 직접 로그인하면 명령이 세션을 감지하고 저장한 뒤 종료됩니다.

로그인 후 브라우저를 닫아도 일반 조회·다운로드는 동작합니다. 세션이 만료되면 다시 `login`을 실행합니다. `login` 출력의 `learningx_token`이 `false`이면 로그인 브라우저에서 **주차학습 (모듈)** 화면을 한 번 연 뒤 `eclass-mcp login --no-launch`를 실행하세요.

```sh
eclass-mcp doctor
```

`doctor`는 저장된 세션으로 프로필 API를 한 번 조회합니다. 이름·학번·쿠키는 출력하지 않습니다.

### 이미 실행 중인 브라우저 사용

```sh
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --remote-debugging-address=127.0.0.1 \
  --remote-debugging-port=9222 \
  --user-data-dir="$HOME/.cau-eclass/chrome-profile" \
  https://eclass3.cau.ac.kr/

eclass-mcp --cdp-endpoint http://127.0.0.1:9222 login --no-launch
```

Chrome 136부터 기본 사용자 데이터 디렉터리에서는 remote debugging 플래그가 적용되지 않습니다. 전용 디렉터리를 사용하세요. 디버깅 포트는 loopback에만 열고 외부에 노출하지 마세요.

### 세션 갱신과 삭제

세션이 만료되면 저장된 endpoint의 열린 브라우저에서 쿠키를 다시 읽습니다. 브라우저가 닫혀 있으면 `eclass-mcp login`을 다시 실행합니다.

```sh
eclass-mcp logout
```

세션 파일은 macOS `~/Library/Application Support/cau-eclass/session.json`, Linux `${XDG_CONFIG_HOME:-~/.config}/cau-eclass/session.json`, Windows `%APPDATA%\cau-eclass\session.json`에 `0600`으로 저장됩니다. `ECLASS_SESSION_FILE`로 위치를 바꿀 수 있습니다.

## 레거시 CDP 어댑터

페이지 안에서 모든 요청을 실행하는 기존 방식이 필요하면:

```sh
eclass-mcp --browser cdp --cdp-endpoint http://127.0.0.1:9222 doctor
```

이 모드는 `cau-eclass-mcp[cdp]`의 Playwright 연결 API를 사용합니다. 브라우저를 시작하거나 종료하지 않고 기존 디버깅 세션에 붙습니다. OZ 계획서 PDF 내보내기와 과제 제출은 HTTP 모드에서도 이 CDP 경로로 위임됩니다.

## 인증과 제한

- 로그인은 사용자가 브라우저에서 직접 합니다. 비밀번호·OTP를 수집하지 않습니다.
- HTTP 모드의 세션 파일은 `.cau.ac.kr` 쿠키만 담는 bearer 자격 증명입니다. 공유·동기화하지 말고, 필요 없으면 `logout`으로 삭제하세요.
- 다운로드는 대상 호스트별 쿠키 범위와 리다이렉트 목적지를 검사합니다. LearningX Bearer 토큰은 다운로드 호스트에 전달하지 않습니다.
- 인증값과 서명 URL은 도구 응답·일반 오류에 노출하지 않습니다.

참고: [Chrome remote debugging 변경](https://developer.chrome.com/blog/remote-debugging-port), [Playwright connect_over_cdp](https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp).
