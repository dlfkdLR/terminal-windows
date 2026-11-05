# 호환성 매트릭스

**실측 결과만 기록합니다.** 컴파일 성공을 플랫폼 지원으로 표기하지 않습니다. 각 칸은 `✅ 날짜/환경`, `❌ 이유`, `⏳ pending`, `⛔ unsupported(의도적)` 중 하나입니다.

| 단계 | macOS Apple Silicon (Win11 ARM64) | macOS Intel (Win11 x64) | Windows x64 + QEMU/WHPX | Windows x64 + Hyper-V |
|---|---|---|---|---|
| 1. 빌드 | ✅ 2026-11-05, macOS 26 arm64, Rust 1.96 | ⏳ CI(macos-13) | ⏳ CI(windows-latest) | ⏳ 백엔드 미구현 |
| 2. CLI 실행 | ✅ 2026-11-05 (`doctor`/`ls`/`status`/`create` 오류 경로/`mcp`) | ⏳ | ⏳ | ⏳ |
| 3. VM 부팅 | 🟡 2026-11-05 UEFI 펌웨어까지 (아래 실측 기록). Windows 미디어로는 미검증 | ⏳ | ⏳ swtpm 부재로 Win11 불가 예상 | ⏳ |
| 4. Windows 설치 | ⏳ | ⏳ | ⏳ | ⏳ |
| 5. 에이전트 연결 | ⏳ | ⏳ | ⏳ | ⏳ |
| 6. 터미널 뷰어 | ⏳ (뷰어 미구현) | ⏳ | ⏳ | ⏳ |
| 7. MCP 제어 | 🟡 2026-11-05 실제 VM(UEFI)에 stdio로 호출 성공. Claude Code/Codex 클라이언트 연동은 ⏳ | ⏳ | ⏳ | ⏳ |
| 8. E2E | ⏳ | ⏳ | ⏳ | ⏳ |

## 실측 기록: Apple Silicon 첫 부팅 (2026-11-05)

환경: MacBook Pro Apple Silicon, macOS 26, QEMU 11.1.3 (Homebrew), swtpm 0.10.2, Homebrew edk2-aarch64(Secure Boot 기능 없음). Windows ISO가 없어 미디어 없이 빈 80 GiB qcow2 디스크로 부팅.

| 항목 | 결과 |
|---|---|
| `doctor` HVF probe | ✅ supported (실제 기동 확인) |
| `create` (qcow2, efivars, TPM 상태 디렉터리, 에이전트 키) | ✅ |
| `start`: HVF + `virt` + ramfb + NVMe + qemu-xhci + swtpm TPM + virtio-serial + NAT + QMP + VNC(unix) | ✅ `running` (아래 수정 1·2 적용 후) |
| UEFI 화면 캡처 (`action screenshot`, QMP screendump PNG) | ✅ 800×600, EDK II 셸, NVMe `BLK0` 인식 |
| 키보드 입력 (`action type`/`key`, QMP qcode) | ✅ 47자 대소문자·기호 문자열 정확히 입력 (이벤트 간격 30 ms) |
| `pause` / 일시정지 중 action 거부 / `resume` | ✅ (거부 시 종료 코드 6) |
| MCP `initialize`(2025-11-25) → `key` + 스크린샷 → `status` | ✅ 실제 VM 대상 |
| `stop` (ACPI) | ✅ 요청 전달. UEFI 셸은 처리할 OS가 없어 계속 실행됨(정상) |
| `stop --force` → swtpm 자동 종료 (`--terminate`) | ✅ 잔여 프로세스 없음 |
| 재시작, TPM 상태 파일 유지 | ✅ |
| 실행 중 `delete` 거부 / 정지 후 삭제 | ✅ |
| QEMU 업그레이드 모의(기록된 Cellar 경로 소실) → 펌웨어 재탐색 | ✅ |
| 마우스(usb-tablet 절대좌표) | ⏳ UEFI 셸에 포인터가 없어 미검증 |

실측으로 발견해 수정한 문제:
1. **`tpm-tis-device` + HVF → `HV_BAD_ARGUMENT`**. TCG에서는 정상이고 HVF에서만 실패. 원인은 PPI RAM 영역(16 KiB 페이지 비정렬)으로 추정되며 `ppi=off`로 해결. Windows 11이 PPI 없이 설치되는지는 ⏳.
2. QEMU 11에서 `-mon` deprecated → `-object monitor-qmp` (QEMU 11 미만은 `-mon` 유지).
3. QMP 소켓은 클라이언트 1개만 받음 → 콘솔 연결을 잡은 채 상태 조회 시 타임아웃. 조회 순서 수정 및 trait 문서화.
4. 키 이벤트를 몰아 보내면 USB HID 큐(16개)가 넘쳐 입력 유실(8 ms 묶음: 유실, 15 ms: 뒤섞임, 30/50 ms: 정확). 기본값 30 ms, `TW_CONSOLE_EVENT_DELAY_MS`로 조정.
5. Homebrew 펌웨어 디스크립터의 버전 포함 경로 → 업그레이드 후 파일명으로 재탐색.

주의: 부팅 직후 EDK II의 5초 `startup.nsh` 카운트다운 중에 입력하면 "아무 키"로 소비됩니다(테스트 타이밍 문제, 코드 결함 아님).

## 자동 테스트 (2026-11-05, Apple Silicon, macOS 26, Rust 1.96)

`cargo test --workspace`: 103개 통과, 0 실패. `cargo clippy --workspace --all-targets -D warnings` 통과, `cargo fmt --check` 통과.
이 테스트는 프로토콜·명령행 생성·모의 QMP·모의 백엔드 수준의 검증이며, **실제 VM 부팅이나 Windows 제어의 증거가 아닙니다.**

게스트 에이전트의 `cfg(windows)` 코드(`win32.rs`, `main.rs`의 windows 경로)는 이 장비에 Windows 타깃 표준 라이브러리가 없어 **로컬 컴파일되지 않았습니다.** CI의 `windows-agent` 잡에서 처음 컴파일됩니다.

## 확인이 필요한 가정 (실측 시 갱신)

| 가정 | 상태 |
|---|---|
| Homebrew `edk2-aarch64` 펌웨어에 Secure Boot 기능 플래그가 있는가 | ✅ 없음(`verbose-static`만). 경고 후 진행. Windows Setup 판정은 ⏳ |
| HVF + `q35,smm=on` (Intel Mac Secure Boot 펌웨어) 동작 | ⏳ |
| WHPX에서 `-cpu max`, `kernel-irqchip=off` 기본값의 타당성 | ⏳ |
| Windows ARM64 설치 프로그램이 `usb-storage` CD를 인식 | ⏳ (UTM 구성 참고) |
| QEMU `screendump format=png` (7.1+) | ✅ QEMU 11.1.3에서 PNG 동작 (PPM 폴백은 미실측) |
| QEMU Windows 빌드의 `-chardev pipe` 서버 동작과 QMP | ⏳ |
| virtio-serial 처리량 | ⏳ 벤치마크 예정 |

## 의도적 미지원

- Apple Silicon에서 x64 Windows (TCG 에뮬레이션) — 거부
- x64 호스트에서 ARM64 Windows — 거부
- tmux/screen 내부 인라인 그래픽 — 네이티브 창 폴백
