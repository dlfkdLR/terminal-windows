# 호환성 매트릭스

**실측 결과만 기록합니다.** 컴파일 성공을 플랫폼 지원으로 표기하지 않습니다. 각 칸은 `✅ 날짜/환경`, `❌ 이유`, `⏳ pending`, `⛔ unsupported(의도적)` 중 하나입니다.

| 단계 | macOS Apple Silicon (Win11 ARM64) | macOS Intel (Win11 x64) | Windows x64 + QEMU/WHPX | Windows x64 + Hyper-V |
|---|---|---|---|---|
| 1. 빌드 | ✅ 2026-11-05, macOS 26 arm64, Rust 1.96 | ⏳ CI(macos-13) | ⏳ CI(windows-latest) | ⏳ 백엔드 미구현 |
| 2. CLI 실행 | ✅ 2026-11-05 (`doctor`/`ls`/`status`/`create` 오류 경로/`mcp`) | ⏳ | ⏳ | ⏳ |
| 3. VM 부팅 | ⏳ QEMU·swtpm 미설치 | ⏳ | ⏳ swtpm 부재로 Win11 불가 예상 | ⏳ |
| 4. Windows 설치 | ⏳ | ⏳ | ⏳ | ⏳ |
| 5. 에이전트 연결 | ⏳ | ⏳ | ⏳ | ⏳ |
| 6. 터미널 뷰어 | ⏳ (뷰어 미구현) | ⏳ | ⏳ | ⏳ |
| 7. MCP 제어 | ⏳ (서버는 가짜 대상으로 테스트됨) | ⏳ | ⏳ | ⏳ |
| 8. E2E | ⏳ | ⏳ | ⏳ | ⏳ |

## 자동 테스트 (2026-11-05, Apple Silicon, macOS 26, Rust 1.96)

`cargo test --workspace`: 102개 통과, 0 실패. `cargo clippy --workspace --all-targets -D warnings` 통과, `cargo fmt --check` 통과.
이 테스트는 프로토콜·명령행 생성·모의 QMP·모의 백엔드 수준의 검증이며, **실제 VM 부팅이나 Windows 제어의 증거가 아닙니다.**

게스트 에이전트의 `cfg(windows)` 코드(`win32.rs`, `main.rs`의 windows 경로)는 이 장비에 Windows 타깃 표준 라이브러리가 없어 **로컬 컴파일되지 않았습니다.** CI의 `windows-agent` 잡에서 처음 컴파일됩니다.

## 확인이 필요한 가정 (실측 시 갱신)

| 가정 | 상태 |
|---|---|
| Homebrew `edk2-aarch64` 펌웨어에 Secure Boot 기능 플래그가 있는가 | ⏳ 없으면 경고 후 진행, Windows Setup 판정 확인 |
| HVF + `q35,smm=on` (Intel Mac Secure Boot 펌웨어) 동작 | ⏳ |
| WHPX에서 `-cpu max`, `kernel-irqchip=off` 기본값의 타당성 | ⏳ |
| Windows ARM64 설치 프로그램이 `usb-storage` CD를 인식 | ⏳ (UTM 구성 참고) |
| QEMU `screendump format=png` (7.1+) | ⏳ PPM 폴백 구현됨 |
| QEMU Windows 빌드의 `-chardev pipe` 서버 동작과 QMP | ⏳ |
| virtio-serial 처리량 | ⏳ 벤치마크 예정 |

## 의도적 미지원

- Apple Silicon에서 x64 Windows (TCG 에뮬레이션) — 거부
- x64 호스트에서 ARM64 Windows — 거부
- tmux/screen 내부 인라인 그래픽 — 네이티브 창 폴백
