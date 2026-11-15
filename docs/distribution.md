# 배포

`eclass-mcp`는 GitHub Releases에서 사전 빌드 실행파일로 배포합니다. macOS Apple Silicon, Linux x86_64·ARM64, Windows x64를 지원합니다.

[릴리스 절차](../rust/RELEASING.md)에 따라 **Rust release** 워크플로를 실행합니다. 각 플랫폼에서 실행파일을 빌드하고 버전과 설정 생성을 검사한 뒤, 압축 파일과 SHA-256을 게시합니다. 게시가 끝나면 macOS·Linux·Windows에서 공개 설치 스크립트를 실행해 검증합니다.

OZ PDF 저장은 설치된 Chromium 계열 브라우저를 사용합니다. e-Class MCP 배포에는 자체 실행파일, 설치 스크립트, 라이선스와 문서가 포함됩니다. 이전 Python MCP 배포는 [PyPI 릴리스 이력](https://pypi.org/project/cau-eclass-mcp/#history)에서 볼 수 있습니다.

라이선스는 프로젝트 전용 `LicenseRef-CAU-EClass-Noncommercial-1.0`이며 상업적 사용을 금지합니다. 배포본에 LICENSE와 NOTICE를 포함합니다.
