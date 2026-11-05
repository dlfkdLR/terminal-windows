# AGENTS.md — terminal-windows 개발 에이전트 지침

이 저장소에서 작업하는 코딩 에이전트(Claude Code, Codex 등)를 위한 규칙입니다. 상위 기획은 `docs/PLAN.md`가 우선합니다.

## 작업 전
- `docs/PLAN.md` §7 단계표에서 현재 단계와 종료 조건을 확인하세요. P8은 P2c·P4·P6 완료 전 착수 금지.
- 수정 후 반드시: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## 반드시 지킬 것
- **실측 없이 지원을 주장하지 않기.** `docs/compatibility.md`에는 실제로 실행한 결과(날짜, 호스트, 명령)만 기록. 컴파일 성공 ≠ 지원.
- `Support::Supported`는 실제 probe로 검증된 능력에만 사용. 나머지는 `Unverified`/`Unsupported`.
- 가짜 테스트, 항상 통과하는 테스트, 빈 placeholder 크레이트 금지.
- 생명주기 로직은 `tw_core::service::VmService`에만. CLI/MCP에 중복 구현 금지.
- 외부 프로세스 인자는 `tw-qemu::cmdline`의 순수 함수로만 생성. 셸 사용 금지, 게스트 데이터로 명령 생성 금지.
- 무제한 큐/버퍼 금지(상한 명시). 네트워크 리스너 추가 금지.
- 개인 경로 하드코딩 금지. `TERMINAL_WINDOWS_HOME`, `TW_QEMU_DIR`, `TW_SWTPM`으로 재정의.
- Windows 요구사항(TPM, Secure Boot, RAM, 정품인증, OOBE) 우회 코드 금지.
- VM 디스크, ISO, 키, 대용량 바이너리 커밋 금지. 원격 push는 명시적 허락 후에만.
- upstream 코드 개작 시 파일 머리말과 `docs/PROVENANCE.md`에 출처 기록.

## 커밋
작은 단위, 접두사 `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `build:`, `ci:`.

## 테스트 장비 메모
- 이 저장소가 iCloud 등 동기화 폴더에 있을 수 있어 빌드 산출물은 `target.nosync/`(`.cargo/config.toml`).
- 게스트 에이전트의 `cfg(windows)` 코드는 Windows 타깃이 있는 환경(CI `windows-agent` 잡)에서만 컴파일됩니다.
