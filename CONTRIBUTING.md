# 개발

```sh
cd rust
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo build --release --locked
```

Chrome·Edge·Chromium이 설치된 환경에서 로컬 합성 OZ 뷰어 테스트를 실행합니다.

```sh
cargo test --locked oz::tests::exports_native_pdf_from_cross_origin_oz_frame -- --ignored
```

실제 OZ PDF 검증은 본인 계정의 강좌에서만 수행하세요. 세션 파일, 쿠키, PDF 본문, 서명 URL은 fixture나 로그에 넣지 마세요. 테스트는 가상 계정과 가상 자료를 사용합니다.

배포 산출물 검증은 [배포 문서](docs/distribution.md), 연동 변경은 [연동 계약](docs/integration.md)을 참고하세요.
