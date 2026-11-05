# terminal-windows 고도화 기획서 (v2)

- 작성일: 2026-11-05
- 기준 upstream: `govijr/terminal-mac` @ `ac787d5` (v0.2.0 build 6, MIT)
- 대상: 원본 구현 프롬프트("Terminal-Windows — Full Implementation Prompt")를 검토하고, **Windows 게스트/Windows 호스트 개발에 바로 들어갈 수 있도록** 수정·보강한 기획

이 문서는 원본 프롬프트를 대체하는 **상위 기획서**입니다. 원본 프롬프트의 요구사항 중 유지되는 것은 그대로 따르고, 이 문서에서 바뀐 부분은 이 문서가 우선합니다. 세부 설계는 다음 문서로 나뉩니다.

| 문서 | 내용 |
|---|---|
| [upstream-analysis.md](upstream-analysis.md) | terminal-mac 실제 코드 분석, 이식/재작성 판정 |
| [architecture.md](architecture.md) | 크레이트 구조, 런타임 구성, 데이터 흐름 |
| [protocol.md](protocol.md) | 호스트↔게스트 프로토콜 v1 |
| [security.md](security.md) | 위협 모델과 기본 보안 정책 |
| [compatibility.md](compatibility.md) | 플랫폼별 검증 매트릭스 (실측만 기록) |
| [PROVENANCE.md](PROVENANCE.md) | upstream에서 복사/개작/신규 작성한 코드 기록 |

---

## 1. 한 줄 요약

> terminal-mac은 "Apple Virtualization + macOS 게스트 + virtio-fs 메일박스 + 게스트 헬퍼" 구조입니다. 이 구조를 Windows로 그대로 옮기면 **전송 계층, TPM, 화면/입력 경로, Windows 호스트 터미널 그래픽**에서 막힙니다. v2 기획은 **하이퍼바이저 제어면(QMP/VNC)** 과 **게스트 에이전트 제어면(virtio-serial)** 을 분리하여, 에이전트 없이도 설치 화면·로그인·UAC까지 보고 조작할 수 있게 하고, 에이전트는 유니코드 입력·UI Automation 같은 "게스트 내부에서만 가능한 일"에 집중시킵니다.

---

## 2. 원본 프롬프트 문제점 분석

심각도: 🔴 설계를 바꾸지 않으면 동작 불가 · 🟠 큰 일정/품질 리스크 · 🟡 명세 모호/누락

### 2.1 🔴 공유폴더 메일박스는 macOS/Windows 호스트에서 성립하지 않음

upstream은 Apple VZ의 virtio-fs 자동 마운트(`macOSGuestAutomountTag`)에 요청/응답 JSON 파일을 떨어뜨리는 방식입니다. QEMU에서 virtio-fs를 쓰려면 호스트에 `virtiofsd`가 필요한데, **virtiofsd는 Linux 전용**입니다(vhost-user + Linux 커널 기능 의존). 즉 목표 호스트 3종(Windows x64, macOS Intel, macOS Apple Silicon) 모두에서 이 경로가 없습니다. 게스트 쪽도 Windows는 virtio-fs 드라이버 + WinFsp 설치가 필요합니다.

**수정:** 게스트 에이전트 전송은 **virtio-serial(virtserialport)** 을 1순위로 합니다. 호스트 쪽은 QEMU chardev(Unix 소켓 / Windows named pipe), 게스트 쪽은 `\\.\Global\com.terminal-windows.agent.0` 장치입니다. 네트워크 리스너가 없고, 호스트 엔드포인트는 파일시스템 권한(0600/DACL)으로 보호됩니다. Hyper-V 백엔드는 **hvsocket(AF_HYPERV)** 을 씁니다. 2순위 폴백은 SLIRP `guestfwd`(`restrict=on`으로 오프라인 모드와 공존 가능)이며, 공유폴더 메일박스는 채택하지 않습니다.

### 2.2 🔴 Windows 호스트에서 QEMU용 vTPM(swtpm)이 사실상 없음

Windows 11 설치에는 TPM 2.0이 필요합니다. QEMU의 TPM은 외부 `swtpm` 프로세스(`-tpmdev emulator`)에 의존하는데, swtpm은 Linux/macOS(Homebrew) 위주이며 **Windows 공식 빌드가 없습니다.** 원본 프롬프트는 "WHPX로 Windows 11 x64 부팅"을 1차 목표로 두지만, 이 조합은 TPM 단계에서 막힐 가능성이 높습니다. 또한 QEMU+WHPX는 Windows 11 게스트에서 성능·안정성 이슈 보고가 많습니다.

**수정:**
- Windows 호스트의 **지원(supported) 경로는 Hyper-V 백엔드**(Gen2 VM, 내장 vTPM, Secure Boot, hvsocket)로 격상합니다. 원본의 "Phase 8 선택 사항"에서 **Phase 3b**로 앞당깁니다.
- QEMU+WHPX는 Windows 호스트에서 `experimental`로 표기하고, swtpm 사용 가능 여부를 `doctor`가 실제로 검사합니다. TPM이 없으면 Windows 11 생성은 **거부**합니다(우회 플래그 제공 안 함 — 원본의 "eligibility check 우회 금지" 준수).
- Windows Home 호스트(Hyper-V 불가)는 "QEMU+WHPX + swtpm이 검증되기 전까지 미지원"으로 명시합니다.

### 2.3 🔴 게스트 에이전트만으로는 설치·로그인·UAC 화면을 다룰 수 없음

Desktop Duplication / Windows Graphics Capture와 `SendInput`은 **사용자 세션의 일반 데스크톱**에서만 동작합니다. 다음 상황에서는 원리적으로 불가능합니다.

| 상황 | 에이전트 캡처 | 에이전트 입력 |
|---|---|---|
| Windows 설치/OOBE (에이전트 미설치) | ✗ | ✗ |
| 로그인/잠금 화면 (Winlogon 데스크톱) | ✗ | ✗ |
| UAC 동의 창 (Secure Desktop) | ✗ | ✗ |
| 관리자 권한 창 (UIPI) | ○ | ✗ (medium IL → high IL 차단) |
| Session 0 서비스로 실행 시 | ✗ | ✗ |

**수정:** **이중 제어면(Two control planes)** 을 도입합니다.

- **Plane H (Hypervisor):** QEMU QMP `screendump`/VNC 프레임버퍼로 화면, QMP `input-send-event`(usb-tablet 절대좌표 + qcode 키)로 입력. 물리 키보드/모니터와 동일한 위치이므로 BIOS·설치·로그인·UAC까지 모두 보입니다. 이는 보안 경계 우회가 아니라 VM 소유자의 "물리 콘솔"입니다.
- **Plane G (Guest agent):** 유니코드 텍스트 입력, UI Automation 트리, 포커스 앱, DPI/해상도, 준비 상태, 정상 종료 요청.
- 스크린샷의 기준 진실(ground truth)은 Plane H입니다. Plane G가 보고하는 데이터는 `source: guest-reported`로 표시합니다(게스트 내부 악성코드가 에이전트를 사칭할 수 있으므로).
- MCP 정책: 게스트가 Secure Desktop/잠금 화면 상태면 에이전트 도구는 기본적으로 **동작을 거부하고 사용자에게 넘기라고 안내**합니다. VM 소유자가 `--allow-console-input`을 명시한 경우에만 Plane H로 조작합니다.

### 2.4 🔴 QEMU는 네이티브 창을 실행 중에 show/hide 할 수 없음

upstream의 `show`/`hide`는 VZ 창을 `orderOut`으로 숨깁니다. QEMU의 `-display cocoa/gtk/sdl` 창은 런타임에 숨기거나 다시 띄울 수 없습니다.

**수정:** VM은 **항상 headless**(`-display none`)로 실행하고, 화면은 VNC(Unix 소켓/로컬 전용)로만 내보냅니다. `show`는 "네이티브 뷰어 프로세스 실행"(외부 VNC 뷰어 또는 향후 자체 뷰어), `hide`는 그 뷰어 종료로 의미를 재정의합니다. VM 생명주기와 창 생명주기가 분리되어 오히려 안정적입니다.

### 2.5 🟠 Windows 호스트 터미널은 Kitty 그래픽을 지원하지 않음

Windows Terminal은 Kitty Graphics Protocol을 지원하지 않고 **Sixel**을 지원합니다(1.22+). Kitty·Ghostty는 Windows 네이티브 빌드가 없습니다. WezTerm은 Windows에서 동작하며 Kitty/iTerm2 이미지 프로토콜을 일부 지원합니다. 또한 upstream의 고속 경로(`t=s` POSIX 공유메모리)는 Windows에 없습니다.

**수정:** 렌더러를 백엔드 플러그형으로 설계합니다: `kitty-shm`(POSIX, Kitty/Ghostty), `kitty-direct`(PNG/RGBA 청크), `iterm2`, `sixel`(Windows Terminal; 저프레임 모드), 그리고 `native-window` 폴백. 지원 여부는 **에뮬레이터 레벨에서 실측**(Kitty `a=q` 질의, DA1 응답의 Sixel 비트 `4`)으로 판정하며 환경변수만으로 판정하지 않습니다.

### 2.6 🟠 좌표계가 정의되지 않음 (Windows DPI 스케일)

Windows는 125%/150% 스케일링이 기본인 경우가 많습니다. 스크린샷 픽셀, 논리 좌표, `SendInput` 절대좌표(0–65535 정규화 가상 데스크톱), QEMU usb-tablet 좌표(0–32767)가 모두 다릅니다. upstream은 macOS "point"로 통일했지만 Windows에는 그대로 대응되는 개념이 없습니다.

**수정:** **공개 좌표계 = 주 디스플레이의 물리 픽셀(스크린샷 이미지 픽셀, 좌상단 원점)** 로 고정합니다. 에이전트는 Per-Monitor-V2 DPI aware 매니페스트로 빌드합니다. 스크린샷을 축소해서 반환할 때는 `scale` 메타데이터를 같이 주고 서버가 역변환합니다(모델이 직접 환산하지 않도록). 변환 함수는 `tw-core::coords`에서 단위 테스트합니다.

### 2.7 🟠 게스트 에이전트의 실행 방식이 모호함

"서비스로 설치하지 말라"는 요구와 "부팅 후 자동 연결" 요구가 충돌합니다. Windows 서비스는 Session 0에서 실행되어 사용자 데스크톱을 캡처/조작할 수 없습니다.

**수정:** 에이전트는 **사용자 로그온 시 실행되는 Scheduled Task(사용자 세션, 대화형)** + **트레이 아이콘(항상 보임)** 으로 동작합니다. 시작 메뉴에 "terminal-windows agent 중지/제거" 항목을 둡니다. 관리자 권한 창 조작이 필요하면 `uiAccess=true` 서명 빌드(Program Files 설치 필요)를 별도 옵션으로 검토하되, 기본은 medium IL이며 UIPI 차단 시 `elevated_target` 오류를 반환합니다.

### 2.8 🟠 Apple Silicon의 Windows 11 ARM64는 드라이버/디스플레이 제약이 큼

- Windows ARM64에는 virtio-gpu 디스플레이 드라이버가 사실상 없어 **ramfb(고정 해상도)** 를 씁니다 → "동적 해상도 적응" 요구를 ARM64에서는 만족할 수 없습니다(펌웨어 GOP 해상도로 고정).
- 디스크는 inbox 드라이버가 있는 **NVMe**를 사용(설치 시 추가 드라이버 불필요).
- 네트워크는 virtio-net 드라이버(virtio-win ARM64)가 필요 → `--drivers-iso`로 virtio-win ISO를 두 번째 CD로 연결.
- Microsoft가 공식 인정한 Apple Silicon용 Windows 11 가상화는 Parallels/VMware Fusion입니다. QEMU 경로는 기술적으로 동작하지만 **Microsoft 공식 지원 구성이 아님**을 문서에 명시합니다.
- x64 Windows on Apple Silicon(TCG 에뮬레이션)은 **거부**합니다(원본 요구 유지).

### 2.9 🟠 Intel Mac + HVF + Secure Boot 펌웨어 조합은 검증되지 않음

Secure Boot 지원 OVMF(`*-secure-code.fd`)는 보통 SMM(`-machine q35,smm=on`)을 요구하는데, HVF 가속기의 SMM 지원 여부는 QEMU 버전별로 실측이 필요합니다. Windows 11 요구사항은 "Secure Boot **capable** UEFI"입니다.

**수정:** 펌웨어 선택은 하드코딩된 경로가 아니라 QEMU가 설치하는 **펌웨어 디스크립터 JSON**(`share/qemu/firmware/*.json`)을 파싱하여 `architecture`·`interface-types`·`features`(secure-boot, enrolled-keys, requires-smm)로 고릅니다(구현 완료: `tw-qemu::firmware`). Intel Mac의 Secure Boot 경로는 compatibility.md에 `pending`으로 기록합니다.

### 2.10 🟠 프로토콜 데드라인에 절대 시각을 쓰면 안 됨

upstream `Request.deadline`은 **호스트 절대 시각**이고 게스트가 `deadline > Date()`로 검사합니다. 게스트 시계는 부팅 직후·일시정지 후·스냅샷 복원 후 크게 어긋나므로 요청이 즉시 만료되거나 영원히 유효해집니다.

**수정:** 프로토콜 v1은 **상대 타임아웃(`timeout_ms`) + 호스트 측 데드라인 관리 + 단조 증가 시퀀스 번호**를 사용합니다(protocol.md).

### 2.11 🟠 "재생 방지·인증"의 실효 범위가 과대평가됨

virtio-serial 포트는 게스트 내부의 **아무 프로세스나** 열 수 있습니다. 게스트를 신뢰할 수 없다는 전제(원본 §10)에서는 "게스트 인증"이 호스트를 보호해주지 못합니다. 실제 보호는 (1) 호스트 엔드포인트 접근 제어, (2) 게스트 입력의 엄격한 검증, (3) 게스트에 위험한 호스트 기능을 노출하지 않는 것에서 나옵니다.

**수정:** VM별 키 기반 HMAC 세션은 유지하되 목적을 정확히 기술합니다 — VM 혼동 방지, 호스트 측 다른 로컬 프로세스의 사칭 방지(키 파일 0600), 재생/순서 공격 방지. 문서에 "게스트 인증 ≠ 게스트 신뢰"를 명시합니다(security.md).

### 2.12 🟡 범위·순서·산출물 정의의 문제

| 문제 | 수정 |
|---|---|
| 11개 크레이트 + "빈 모듈 금지" 요구가 충돌 (예: `tw-hyperv`를 미리 만들면 빈 껍데기) | 실제 코드가 생길 때만 크레이트를 추가. 현재 7개 + 게스트 에이전트 1개 |
| `tw-transport`·`tw-security`가 독립 크레이트일 실익이 작음 | 전송/세션 보안은 `tw-protocol`, 파일 권한·키 관리는 `tw-core::secure`로 통합 |
| 동기 trait 예시(`fn start(...)`)와 Tokio 요구 불일치 | `async_trait` 기반 객체 안전 trait + `Support::{Supported, Unsupported, Unverified}` |
| E2E(설치에 수 시간, 정품 ISO 필요)를 단일 세션 완료 조건으로 둠 | 단계별 **종료 조건(exit criteria)** 과 "실측 증거" 형식을 정의(§7) |
| MCP 도구에 `double_click`/`right_click`/`wait`/UI 트리/화면 일부 확대가 없음 | `click{button,count}`, `wait_for_change`, `ui_tree`(Plane G), `screenshot{region,max_width}` 추가 |
| MCP 프로토콜 버전 미지정 | 클라이언트가 요청한 버전 중 지원 목록(`2025-11-25`, `2025-06-18`, `2025-03-26`)과 협상, `structuredContent` 사용 |
| `delete` 안전성만 언급, 실행 중 삭제/부분 삭제 처리 없음 | 실행 중이면 거부, 잠금 획득 후 `machine.toml` 먼저 `.deleting`으로 이름 변경 후 삭제 |
| 무인 설치(autounattend)에 대한 입장이 없음 | 공식 문서화된 `autounattend.xml`(로컬 계정, 에이전트 설치 FirstLogonCommands)은 **명시적 opt-in**으로 제공. TPM/Secure Boot/네트워크 요구 우회 키(LabConfig, BypassNRO 등)는 넣지 않음 |
| 이미지 키보드 레이아웃 미고려 | Plane H 텍스트 입력은 US 레이아웃 ASCII만 보장(qcode는 물리 키). 그 외 문자는 Plane G 유니코드 입력 필요 — 명시적 오류로 안내 |

---

## 3. 제품 정의 (v2)

### 3.1 목표
1. 터미널 한 줄(`terminal-windows up`)로 Windows VM을 만들고 부팅하고 본다.
2. 설치부터 로그인까지 **에이전트 없이** 터미널/에이전트가 화면을 보고 조작할 수 있다(Plane H).
3. 로그인 후에는 에이전트가 유니코드 입력·UI 트리 등 고급 기능을 제공한다(Plane G).
4. CLI·MCP·뷰어가 **하나의 `VmService`** 를 공유한다.

### 3.2 비목표 (명시)
- 하드닝된 악성코드 샌드박스가 아님.
- Windows 라이선스/정품인증/하드웨어 요구 우회 없음. ISO/펌웨어 번들 없음.
- 클라우드 서비스·유료 API 의존 없음.
- x64 게스트를 Apple Silicon에서 에뮬레이션하지 않음.

### 3.3 지원 등급 정의
| 등급 | 의미 |
|---|---|
| `supported` | compatibility.md의 8단계 실측이 모두 통과한 조합 |
| `experimental` | 빌드·CLI는 동작, VM 부팅 이상 일부 미검증 |
| `pending` | 코드 경로는 있으나 실측 환경 없음 |
| `unsupported` | 의도적으로 거부(오류 메시지에 이유 포함) |

---

## 4. 아키텍처 개요

```text
                ┌────────────── terminal-windows (host) ─────────────┐
 CLI ──┐        │                                                      │
 MCP ──┼──► VmService ──► VirtualizationBackend (QEMU | Hyper-V)       │
 View ─┘        │   │                                                  │
                │   ├──► ComputerControl ── Plane H: QMP / VNC ───────┼──► QEMU ──► 가상 모니터/키보드/태블릿
                │   │                    └─ Plane G: tw-protocol ─────┼──► virtio-serial / hvsocket
                │   └──► Store (machine.toml, 잠금, 키, 로그)          │            │
                └──────────────────────────────────────────────────────┘            ▼
                                                          Windows 게스트: tw-agent (사용자 세션, 트레이 표시)
                                                          Desktop Duplication · SendInput · UI Automation
```

| 호스트 | 백엔드 | 가속 | 게스트 | 펌웨어/TPM | Plane G 전송 | 등급 목표 |
|---|---|---|---|---|---|---|
| macOS Apple Silicon | QEMU `virt` | HVF | Win11 ARM64 | edk2-aarch64 + swtpm(`tpm-tis-device`) | virtio-serial (Unix 소켓) | supported (1순위) |
| macOS Intel | QEMU `q35` | HVF | Win11 x64 | OVMF + swtpm(`tpm-tis`) | virtio-serial | experimental → supported |
| Windows x64 (Pro+) | Hyper-V Gen2 | Hyper-V | Win11 x64 | 내장 UEFI/vTPM | hvsocket | supported (Phase 3b) |
| Windows x64 | QEMU `q35` | WHPX | Win11 x64 | OVMF + swtpm(가용 시) | virtio-serial (named pipe) | experimental |

개발 1순위 호스트를 **Apple Silicon**으로 두는 이유: 현재 개발 장비이며, QEMU+HVF+swtpm+edk2가 모두 Homebrew로 갖춰지고, Plane H/G 설계를 가장 빨리 실측 검증할 수 있습니다. 동일 코드가 Windows 호스트 QEMU 경로에도 그대로 쓰입니다(차이는 소켓→named pipe, 가속기 이름뿐).

자세한 내용: [architecture.md](architecture.md)

---

## 5. Windows 게스트 에이전트 설계 요점

- **언어/빌드:** Rust, `windows-sys`, 타깃 `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`. 매니페스트: PerMonitorV2 DPI aware, `asInvoker`.
- **실행:** 사용자 로그온 Scheduled Task, 트레이 아이콘 상시 표시, 단일 인스턴스 mutex, 로그 `%LOCALAPPDATA%\terminal-windows-agent\agent.log`.
- **설치:** VM별로 생성되는 "tools ISO"(에이전트 exe, VM 키, 설치 스크립트)를 CD로 연결. 사용자가 직접 `install.cmd` 실행, 또는 opt-in autounattend의 FirstLogonCommands.
- **기능 매핑:**

| 기능 | API | 비고 |
|---|---|---|
| 화면 캡처 | Desktop Duplication(`IDXGIOutputDuplication`) | 변경 영역(dirty rects) 기반, 기본 비활성(Plane H 우선) |
| 마우스 | `SendInput` + `MOUSEEVENTF_ABSOLUTE|VIRTUALDESK` | 물리 픽셀 → 0–65535 정규화 (`tw-agent::coords`) |
| 키 조합 | `SendInput` VK + 스캔코드 | `win`, `ctrl`, `alt`, `shift` |
| 유니코드 입력 | `SendInput` `KEYEVENTF_UNICODE` | UTF-16 서로게이트 쌍 처리, 클립보드 미사용 |
| 포커스/UI 트리 | UI Automation (`IUIAutomation`) | 깊이·노드 수 제한 |
| 디스플레이 | `EnumDisplaySettingsEx`, `GetDpiForMonitor` | 해상도/스케일 보고 |
| 세션 상태 | `OpenInputDesktop` 실패/이름 검사, `WTSQuerySessionInformation` | `secure_desktop`, `locked`, `no_session` 보고 |

---

## 6. 보안 기본값 요약 (상세: security.md)

- 호스트 데스크톱/클립보드/홈 디렉터리/자격증명 공유 없음. 공유폴더 기능 자체가 기본 off.
- 네트워크 리스너 없음: QMP·VNC·에이전트 채널은 모두 **Unix 소켓(0600, 디렉터리 0700)** 또는 **현재 사용자 DACL named pipe**. TCP가 불가피한 경우 `127.0.0.1` + VNC 비밀번호 + 무작위 포트, 그리고 compatibility.md에 "로컬 다른 사용자에게 노출" 경고.
- 네트워크 모드: `nat`(기본, 격리 아님을 명시) / `offline`(NIC 없음 또는 `restrict=on`).
- 게스트 문자열은 로그에 기록 시 길이 제한·제어문자 이스케이프. 키/토큰은 로그 금지.
- 호스트는 게스트가 보낸 데이터로 **어떠한 셸 명령도 실행하지 않음**. QEMU/swtpm 실행 인자는 호스트 설정에서만 생성.

---

## 7. 단계별 개발 계획 (수정본)

각 단계는 **종료 조건**을 충족하고 그 증거(명령, 출력, 날짜, 호스트 사양)를 compatibility.md에 기록해야 다음 단계로 넘어갑니다.

| 단계 | 내용 | 종료 조건 | 상태 |
|---|---|---|---|
| P0 | upstream 분석, 라이선스 | upstream-analysis.md, PROVENANCE.md | ✅ 완료 |
| P1 | Rust 기반: 설정 모델, Store, 오류/종료코드, 백엔드 trait, CLI 골격, 로깅 | `cargo test` 통과, `doctor`/`create`/`ls`/`config` 실제 동작 | ✅ 기초 완료 |
| P2a | QEMU 탐지·가속기·펌웨어 디스크립터·커맨드라인 생성·QMP 클라이언트 | 명령행 단위테스트 + 모의 QMP 통합테스트 | ✅ 기초 완료 |
| P2b | 실제 부팅 (Apple Silicon) | `start` → UEFI 화면 스크린샷(QMP) 획득, `pause/resume/stop` 실측 | ⏳ QEMU 설치 후 |
| P2c | Plane H 제어 | ISO 부팅 → 설치 화면을 `action screenshot/click/key`로 진행 | ⏳ |
| P3a | 프로토콜 v1 + 전송 | 핸드셰이크·HMAC·재생방지·크기제한 테스트(완료), virtio-serial 실측 | 🟡 프로토콜 완료, 실측 대기 |
| P3b | Hyper-V 백엔드 (Windows 호스트) | Gen2+vTPM VM 생성, hvsocket 연결 | ⏳ Windows 장비 필요 |
| P4 | Windows 에이전트 | 로그인 후 `action type "한글 테스트"`가 메모장에 입력됨 | 🟡 순수 로직 완료, Win32 부분 CI 컴파일 |
| P5 | 터미널 뷰어 | Kitty/Ghostty/WezTerm/Windows Terminal(sixel) 실측, 리사이즈·복구 | 🟡 레이아웃/인코더 완료 |
| P6 | MCP | observe→act→verify 루프를 Claude Code/Codex에서 실측 | 🟡 서버·스키마 완료 |
| P7 | 크로스플랫폼 검증 | compatibility.md 8단계 매트릭스 | ⏳ |
| P8 | 스냅샷(qcow2 internal / Hyper-V checkpoint), 템플릿, 업데이트, 릴리스 자동화 | P2c·P4·P6 완료 후에만 착수 | ⛔ |

### Windows 버전 개발 착수 순서 (실무용)
1. Windows 11 Pro x64 개발 장비 준비: Hyper-V + Windows Hypervisor Platform 활성화, Rust(MSVC), QEMU for Windows.
2. `cargo test --workspace`와 `cargo build -p tw-agent` 통과 확인(CI와 동일).
3. P3b: `tw-hyperv` 크레이트 추가 → `VirtualizationBackend` 구현(PowerShell `Hyper-V` 모듈을 **고정된 인자 배열**로 호출하거나 WMI `root\virtualization\v2`). 스크린샷은 `Msvm_VirtualSystemManagementService.GetVirtualSystemThumbnailImage`로 먼저 시작하고 성능 측정.
4. 에이전트: `guest/windows-agent`의 `cfg(windows)` 부분을 실제 VM에서 검증 → hvsocket 리스너 추가.
5. 뷰어: Windows Terminal에서 DA1 Sixel 비트 확인 후 sixel 렌더러 구현, WezTerm에서 kitty-direct 실측.

---

## 8. 리스크 레지스터

| ID | 리스크 | 영향 | 대응 |
|---|---|---|---|
| R1 | Windows용 swtpm 부재 | Win 호스트 QEMU 경로 Win11 불가 | Hyper-V 경로 우선, QEMU는 experimental |
| R2 | QEMU+WHPX 성능/안정성 | Win 호스트 체감 품질 | 벤치마크 후 기본 백엔드 결정 |
| R3 | ARM64 ramfb 고정 해상도 | 동적 해상도 불가 | 생성 시 해상도 지정, 문서화 |
| R4 | Microsoft의 OOBE 로컬 계정 경로 축소 | 무인 설치 실패 | 무인 설치는 opt-in, 수동 경로 항상 유지 |
| R5 | virtio-serial 처리량 | Plane G 프레임 스트리밍 한계 | 프레임은 Plane H(VNC) 사용, Plane G는 제어 위주 |
| R6 | Windows Terminal sixel 성능 | 저프레임 | 변경 시에만 전송, 다운스케일, 네이티브 창 폴백 |
| R7 | QEMU Windows 빌드의 AF_UNIX 지원 범위 | 소켓 경로 차이 | named pipe chardev / localhost+비밀번호 폴백, 실측 후 결정 |
| R8 | 게스트 내부 악성코드의 에이전트 사칭 | 잘못된 관측 | 기준 진실은 Plane H, 게스트 보고값 표시 |

---

## 9. 현재 기초 작업 결과 (이 커밋 기준)

구현되어 테스트로 검증된 것과 아직 실측되지 않은 것을 구분합니다.

**구현 + 자동 테스트 통과 (Apple Silicon, macOS 26, Rust 1.96):**
- `tw-core`: VM 이름 검증, `machine.toml` 모델과 Windows 11 요구사항 검증, 호스트/게스트 아키텍처 정책, Store(원자적 저장·배타 잠금·0700/0600 권한), 오류 코드/종료 코드, 좌표 변환, 백엔드/제어 trait, VM별 키 생성.
- `tw-qemu`: 바이너리·가속기·swtpm 탐지, 펌웨어 디스크립터 파싱/선택, QEMU·swtpm·qemu-img 명령 생성(x86_64/aarch64), 비동기 QMP 클라이언트, QemuBackend 생명주기, Plane H 제어(screendump, 절대좌표 마우스, qcode 키, US-ASCII 텍스트).
- `tw-protocol`: v1 프레이밍, 핸드셰이크, HMAC-SHA256 세션, 재생 방지, 크기 제한.
- `tw-input`: upstream TerminalInput 이식, 키 조합 파서, qcode/Windows VK 매핑.
- `tw-renderer`: upstream ImageLayout/TerminalMetricFilter 이식, Kitty 인코더, 터미널 탐지.
- `tw-mcp`: MCP stdio 서버(초기화·버전 협상·도구 스키마·도구 실행), 가짜 제어로 통합 테스트.
- `tw-cli`: `doctor`, `create`, `ls`, `status`, `config`, `start`, `stop`, `pause`, `resume`, `delete`, `logs`, `action ...`, `mcp`.
- `guest/windows-agent`: 요청 디스패처, 좌표 정규화, UTF-16 청킹 등 플랫폼 독립 로직 + 테스트. Win32 `SendInput` 구현은 `cfg(windows)`이며 **로컬에서 컴파일 불가 → GitHub Actions windows-latest에서 컴파일 검증 예정**.

**미실측(정직하게 표기):** QEMU 실제 부팅, Windows 설치, 에이전트 연결, 터미널 그래픽 실표시, MCP 클라이언트 실연동. 이 개발 장비에는 QEMU/swtpm이 설치되어 있지 않습니다(`brew install qemu swtpm` 필요).
