# 아키텍처

## 크레이트

| 크레이트 | 책임 | 의존 |
|---|---|---|
| `tw-core` | VM 이름, `machine.toml` 모델·검증(Win11 요구사항), 호스트/아키텍처 정책, Store(잠금·원자적 저장·권한), 오류/종료코드, 좌표 변환, `VirtualizationBackend`·`ComputerControl` trait, **`VmService`** | — |
| `tw-input` | 터미널 입력 파서(upstream 이식), 키 조합 → QEMU qcode / Windows VK, US-ASCII 키 매핑 | — |
| `tw-protocol` | 게스트 프로토콜 v1: 프레이밍, 상호 인증 핸드셰이크, HMAC 세션, 재생 방지 | — |
| `tw-qemu` | QEMU 탐지·가속기 실측, 펌웨어 디스크립터, 명령행 생성, QMP 클라이언트, `QemuBackend`, Plane H 콘솔 | core, input |
| `tw-renderer` | 레이아웃, 응답 필터, Kitty 인코더, 터미널 능력 판정 | — |
| `tw-mcp` | MCP stdio 서버(`Target` trait, `VmService` 구현) | core |
| `tw-cli` | `terminal-windows` 바이너리 | 위 전부 |
| `tw-agent` (`guest/windows-agent`) | Windows 게스트 에이전트 | core, input, protocol |

원본 프롬프트의 `tw-transport`·`tw-security`는 실익이 작아 각각 `tw-protocol`, `tw-core::secure`로 통합했습니다. `tw-hyperv`는 코드가 생길 때(P3b) 추가합니다(빈 크레이트 금지).

## 단일 서비스 계층

```text
tw-cli ─┐
tw-mcp ─┼─► VmService ─► Arc<dyn VirtualizationBackend> ─► QemuBackend (향후 HyperVBackend)
viewer ─┘        │
                 └─► Store
```

생명주기 규칙(실행 중 삭제 거부, preflight 후 디렉터리 생성, 부분 생성물 보존, 잠금)은 `VmService` 한 곳에만 있습니다.

## 런타임 구성 (QEMU, macOS)

```text
<data>/machines/dev/            machine.toml, disk.qcow2, efivars.fd, firmware.json, tpm/, agent.key, runtime.log, swtpm.log
<runtime>/  (0700)              qmp.sock, vnc.sock, agent.sock, swtpm-ctrl.sock, qemu.pid
```

`<runtime>`은 기본 `<vm>/run`, Unix 소켓 경로 104바이트 제한을 넘으면 `$TMPDIR/tw-<hash>`로 자동 전환합니다.

`start`:
1. 설정 로드·잠금 → 상태가 Stopped인지 확인
2. 기록된 펌웨어(`firmware.json`) 확인, 첨부 미디어 존재 확인
3. `swtpm socket --tpm2 --terminate` 분리 실행, 제어 소켓 대기
4. QEMU를 `-daemonize`로 실행 → 초기화 오류를 동기적으로 수집해 `runtime.log`에 기록
5. QMP `query-status`로 상태 반환

Windows 호스트는 `-daemonize`가 없으므로 분리 프로세스로 실행하고 QMP(named pipe)를 폴링합니다.

## 제어면

| | Plane H (구현됨) | Plane G (프로토콜·에이전트 로직 구현, 호스트 연결 미구현) |
|---|---|---|
| 경로 | QMP `screendump`, `input-send-event` | virtio-serial + tw-protocol |
| 동작 범위 | 펌웨어, 설치, 로그인, UAC 포함 전부 | 로그인된 사용자 데스크톱 |
| 텍스트 | US 레이아웃 ASCII만 (물리 키) | 유니코드 |
| 좌표 | 프레임버퍼 픽셀 → 0..32767 | 물리 픽셀 → 0..65535 |
| 신뢰 | 기준 진실 | guest-reported |

향후 `ComputerControl`을 합성하는 `HybridControl`이 에이전트 연결 시 텍스트/UI 트리는 Plane G, 나머지는 Plane H로 라우팅합니다(P4).

## 확장 지점

- 새 백엔드: `VirtualizationBackend` 구현(`preflight`, `create`, `start`, `stop`, `pause`, `resume`, `status`, `console`, `capabilities`).
- 새 렌더러 전송: `tw-renderer`에 모듈 추가, `detect::decide`에 판정 추가.
