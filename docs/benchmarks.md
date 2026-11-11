# 개선 전후 측정

2026-11-12, 0.4.0 작업 트리와 0.3.0(`fd509fa`)을 같은 장비에서 비교했다. **실제 학교 서버 응답 시간이나 사람의 로그인 시간을 측정한 것이 아니다.** 서버 시작·메모리·요청 수·합성 지연을 분리해 측정했다.

## 환경

- Apple M3 Max, macOS 27.0.1
- Python 3.12.15, FastMCP 4.1.4
- Google Chrome 157.0.8103.40
- Aside 1.26.1111.1046
- 0.3.0: `fd509fa` worktree, `PYTHONPATH`로 분리 실행
- 0.4.0: 현재 작업 트리

## 서버 시작과 상주 메모리

`list_tools`까지의 시간을 3회 측정한 중앙값. lifespan에서 전송을 여는 비용을 포함하며, e-Class API 호출은 없다.

| 모드 | 시작 시간 | 서버 프로세스 트리 RSS | 프로세스 수 |
|---|---:|---:|---:|
| 0.3.0 Aside | 0.67초 | 231 MiB | 2 |
| 0.3.0 CDP | 0.78초 | 255 MiB | 2 |
| **0.4.0 HTTP** | **0.58초** (0.57~0.60) | **106~111 MiB** | **1** |

- HTTP 모드는 Aside 대비 RSS **-52%**, CDP 대비 **-57%**.
- 시작 시간은 Aside 대비 **-15%**, CDP 대비 **-27%**. Python/FastMCP import 자체가 약 0.5초를 차지하므로 절대값은 크게 줄지 않았다.
- CDP 모드의 서버 트리에는 Playwright 드라이버만 포함된다. 별도로 띄운 headless Chrome 트리는 **674.2 MiB / 7 프로세스**였다. 이 브라우저는 서버 자식이 아니므로 위 표에 합산하지 않았다. Aside 앱 자체 메모리도 제외했다.
- RSS는 `ps`의 프로세스 트리 합산이라 공유 메모리를 중복 계산할 수 있다.

재현:

```sh
python scripts/benchmark_server.py --label http --runs 3 \
  --env ECLASS_BROWSER=http --env ECLASS_SESSION_FILE=/path/session.json

# 0.3.0 worktree
python scripts/benchmark_server.py --label aside --runs 3 \
  --cwd /path/to/0.3.0 --env PYTHONPATH=/path/to/0.3.0/src \
  --env ECLASS_BROWSER=aside
```

## 요청 수와 합성 지연

6개 과목, 과목별 1페이지 응답, 과목별 todo 1개·공지 1개, 요청당 고정 50ms 지연의 합성 모델. 동일한 HTTP transport에 대해 (a) 기존 방식의 순차 조회, (b) 새 `get_briefing`, (c) 캐시 적중을 비교했다. **실제 e-Class 지연이 아니라 동시성·캐시 효과만 분리한 값이다.**

| 방식 | 요청 수 | 시간 |
|---|---:|---:|
| 기존 방식: courses + 과목별 assignments + announcements | 13 | 0.679초 |
| `get_briefing` (캐시 없음) | 8 | 0.212초 |
| `get_briefing` (20초 캐시, 두 번째 호출) | 0 | 0.0011초 |

- 요청 수 **-38%**, 합성 지연 **3.2배 개선**.
- 캐시가 따뜻한 두 번째 호출은 학교 요청 없이 즉시 반환.
- 공지 조회는 세마포어 4로 제한해 과목 수가 늘어도 동시 요청을 제한한다.

## UX 비교

| 항목 | 0.3.0 | 0.4.0 |
|---|---|---|
| 핵심 설정 단계 | 5~6단계 | **3단계** |
| 설치 | `[browser]` + Playwright | 기본 설치 |
| 브라우저 실행 | 전용 프로필 + 디버깅 플래그 | `eclass-mcp login`이 대신 실행 |
| 로그인 후 | e-Class 탭 유지 | 브라우저를 닫아도 조회·다운로드 가능 |
| 실행 중 브라우저 | 필수(Aside 또는 CDP) | 일반 조회·다운로드에는 불필요 |
| 세션 지속 | 탭/Aside 상태에 의존 | `session.json`으로 재시작 후에도 유지 |
| 만료 복구 | 탭을 다시 열고 수동 확인 | `eclass-mcp login` 또는 열린 브라우저에서 자동 갱신 |
| 요약 | 여러 도구를 순차 호출 | `get_briefing` 1회 |
| 도구 수 | 24 | 26 |
| 실패 표시 | 목록별 오류 | `complete`/`warnings`로 부분 실패 표시 |
| 보안 경계 | 쿠키가 브라우저 밖으로 나가지 않음 | `0600` 세션 파일( bearer 자격 증명), `logout` 제공 |

브라우저가 필요한 작업은 0.4.0에서도 `download_syllabus_pdf`(OZ 네이티브 PDF)와 `submit_assignment` 두 가지다.

## 해석

- **가장 큰 개선은 메모리 숫자 자체보다 브라우저 의존성 제거**다. CDP는 서버 트리 외에 674 MiB 규모의 Chrome을 요구하고, Aside는 사용자의 Aside 앱과 탭 상태에 의존한다.
- 서버 시작 시간은 Python/FastMCP 고정 비용이 커서 15~27% 개선에 그쳤다. 단일 바이너리로 가면 이 고정 비용을 크게 줄일 수 있다. Go/Rust 이전 검토는 [portability.md](portability.md)에 정리했다.
- 요약 도구는 요청 수를 13→8로 줄이고, 병렬 조회로 합성 지연을 3.2배 줄였다. 캐시는 반복 조회를 거의 무료로 만든다.
- 실제 e-Class 네트워크 지연, 사람의 로그인 시간, OZ PDF 내보내기, 과제 제출, 대용량 다운로드는 측정하지 않았다. 따라서 이 수치는 특정 시점의 로컬 측정값이며, 다른 작업 부하나 실제 서버 성능으로 일반화하지 않는다.
