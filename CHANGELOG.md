# Changelog

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
