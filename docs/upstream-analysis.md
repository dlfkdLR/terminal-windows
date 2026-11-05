# terminal-mac upstream 분석

- 대상: `govijr/terminal-mac` @ `ac787d5` (v0.2.0 build 6), Swift 6, 약 3,500줄
- 라이선스: MIT (Copyright (c) 2026 Govijr) — 개작 시 저작권·허가 고지 유지 필요. [PROVENANCE.md](PROVENANCE.md)에 기록.
- 분석 방법: README가 아닌 실제 소스 전체를 읽고 판정.

## 1. 모듈 책임과 의존 관계

```text
TerminalMac (실행 파일, macOS 전용 프레임워크 다수)
 ├─ CLI.swift            명령 디스패치, 프로세스 분리(__run), 호스트 메일박스 호출
 ├─ VirtualMachine.swift VZ 설정/설치/런타임, 호스트 제어 메일박스 루프, NSWindow
 ├─ Viewer.swift         터미널 뷰어 상태기계, 입력 큐(64), 팔레트, 크롬 UI
 ├─ DesktopRenderer.swift 프레임 디코드/합성/인코드(Kitty shm, Kitty PNG, iTerm2)
 ├─ AgentServer.swift    MCP stdio 서버 (도구 8개)
 ├─ Guest.swift          게스트 헬퍼: 메일박스 폴링, CGEvent 입력, 권한 UI
 ├─ GuestCapture.swift   ScreenCaptureKit 스트림, lz4 BGRA 인코딩
 ├─ GuestBootstrap.swift 게스트 내 자기 설치/업데이트/로그인 항목 등록
 ├─ FirstRun.swift / RestoreDownload.swift  up 흐름, IPSW 다운로드
 └─ Entry.swift
TerminalUI   TerminalWriter(직렬 출력 큐), TerminalMetrics(레이아웃·응답 필터), Chrome
TerminalMacCore  Models(인자 파서, Machine, Request/Reply, Store), Mailbox, TerminalInput, Setup(FileLock, readiness, plan)
CPhoneShim   shm_open/shm_unlink C 래퍼 (Swift에서 가변인자 호출 불가)
```

의존 방향: `TerminalMac → TerminalUI, TerminalMacCore, CPhoneShim`. Core/UI는 서로 독립.

## 2. 플랫폼 독립 알고리즘 (이식 가치 있음)

| 항목 | 위치 | 판정 |
|---|---|---|
| VM 이름 규칙 `^[a-zA-Z0-9][a-zA-Z0-9_-]{0,47}$` | Models.swift | 이식 (+Windows 예약어) |
| 터미널 입력 파서(UTF-8 조각, CSI, SGR 마우스, Ctrl+] 종료) | TerminalInput.swift | 이식·확장 |
| 셀↔픽셀 레이아웃, 셀 중심 역매핑 | TerminalMetrics.swift | 이식 |
| 터미널 응답 필터(CSI 14t/16t, Kitty APC 응답) | TerminalMetrics.swift | 이식·확장(DA1) |
| 최신 프레임만 유지하는 렌더 큐(백프레셔) | DesktopRenderer.swift | 개념 이식 (P5) |
| Kitty 이중 버퍼(이미지 id 2/3) + z-index | DesktopRenderer.swift | 이식 |
| 변경 시에만 프레임 전송, 15초 무시청 시 캡처 중지 | GuestCapture.swift | 개념 이식 (P5) |
| 부분 설치물 보존, 덮어쓰기 금지 | VirtualMachine.swift | 원칙 이식 |

## 3. macOS 종속 요소 (재작성 필요)

Virtualization.framework(VZ*), AppKit/NSWindow, ScreenCaptureKit, CGEvent/AX 권한, ServiceManagement, IPSW 복원 이미지, `hw.model == VirtualMac*` 게스트 가드, POSIX 공유메모리(`t=s`), Darwin `flock`/`openat`.

## 4. 프로토콜 형식

- 요청: `request-<UUID>.json` = `{id, action, arguments: {String:String}, deadline: Date}`
- 응답: `reply-<UUID>.json` = `{id, ok, values: {String:String}, error}`
- 프레임: `image-<UUID>.bin` (lz4 BGRA) / `.png`, 메타데이터는 values 문자열
- 폴링: 호스트 2ms→25ms 백오프, 게스트 3ms(활성)/40ms(유휴)

**평가:** 파일 경계 방어(O_NOFOLLOW, nlink==1, 크기 제한, 원자적 rename)는 훌륭합니다. 그러나 (1) 모든 값이 문자열 맵이라 타입 검증이 소비 측에 흩어짐, (2) 절대 시각 deadline은 게스트 시계 어긋남에 취약, (3) 버전 필드 없음, (4) 인증 없음(공유폴더 접근 권한이 곧 권한), (5) **virtio-fs는 QEMU에서 Linux 호스트 전용 virtiofsd가 필요** → Windows/macOS 호스트에서 재사용 불가. ⇒ **재설계** ([protocol.md](protocol.md)).

## 5. 터미널 그래픽

전송 3종: Kitty 공유메모리(raw RGBA, pty 우회), Kitty PNG 청크, iTerm2 OSC 1337. 공유메모리 지원은 1×1 프로브 이미지로 실측(`a=q,t=s` → `OK`). 이미지는 셀 배경 아래(z=-1073741825)에 두어 오버레이 UI가 위에 보임. 출력은 단일 직렬 큐로 이스케이프 섞임 방지, `writeAndWait`로 백프레셔.

Windows 시사점: Windows Terminal은 Kitty 미지원(Sixel 지원), POSIX shm 없음 → PNG 청크 + Sixel 경로가 필요.

## 6. 입력 처리

뷰어는 SGR 마우스(1002/1006)와 대체 화면(1049)을 켜고, 단일/더블/트리플 클릭을 `count`로 이어 보냄. 입력 큐 64개 상한. 텍스트는 연속 입력을 묶어 `type`으로 전송. Cmd 키는 터미널이 가로채므로 Ctrl+G 팔레트로 macOS 단축키 제공. → Windows에서는 Win 키·Ctrl+Alt+Del 팔레트가 필요(Ctrl+Alt+Del은 Plane H로만 가능).

## 7. MCP 도구

`screenshot, click(button, clicks 1-3), move, drag(toX,toY), scroll(dy,dx), type, key, status`, 각 도구에 `screenshot: bool`(행동 후 400ms 대기 후 캡처). 프로토콜 버전은 클라이언트 값을 그대로 반사(협상 없음), 도구 인자 검증은 부분적(`additionalProperties:false` 선언만, 서버 측 검증 없음). → 서버 측 엄격 검증, 버전 협상, `structuredContent`, 생명주기 도구(list/start/stop) 추가.

## 8. VM 생명주기

`start`가 자기 자신을 `__run NAME`으로 분리 실행, 이 프로세스가 VZ VM과 `runtime.lock`을 소유. 생명주기 명령은 `host-control` 메일박스로 전달. QEMU에서는 QEMU 프로세스 자체가 VM이고 QMP가 제어 채널이므로 별도 상주 프로세스가 필요 없음 → 구조 단순화.

## 9. 게스트↔호스트 통신

게스트 헬퍼는 공유폴더 `control`(rw)·`tools`(ro)만 봄. 업데이트는 `tools`에 새 빌드가 놓이면 자기 교체. → Windows에서는 tools를 VM별 ISO(CD)로 제공하고, 자동 업데이트는 P8.

## 10. 보안 가정

호스트 입력/화면 API 미사용, 네트워크 리스너 없음, 게스트 제공 내용 실행 안 함, NAT는 격리 아님 명시, 게스트 권한(TCC)은 사용자가 직접 부여. 모두 유지. 추가로 Windows에서는 Secure Desktop/UIPI/Session 0 경계를 명시적으로 다룸.

## 11. 기존 테스트

XCTest 14개: 설치 계획, readiness, 잠금(심링크 거부 포함), 이름 검증, 인자 파서, 메일박스 경계(순회/심링크/하드링크/크기/비정규파일), 만료·불일치 요청 폐기, 왕복, 타임아웃 정리, 입력 파서 2개. upstream 스스로 "라이브 게스트 검증 미완료"를 명시(docs/VALIDATION.md) — 이 정직성 원칙을 compatibility.md에 그대로 채택.

## 12. 재작성 vs 이식 요약

| 영역 | 판정 |
|---|---|
| VM 엔진 (VZ → QEMU/Hyper-V) | 재작성 |
| 게스트 전송 (virtio-fs 메일박스 → virtio-serial/hvsocket + 인증 프로토콜) | 재작성 |
| 게스트 헬퍼 (ScreenCaptureKit/CGEvent → DXGI/SendInput/UIA) | 재작성 |
| 화면/입력 1차 경로 | **신규: 하이퍼바이저 콘솔(QMP)** — upstream에 없는 계층 |
| 터미널 입력 파서, 레이아웃, 응답 필터 | 이식 |
| 렌더러 전송 정책(최신 프레임만, 이중 버퍼) | 개념 이식 |
| MCP 서버 | 재작성(동일 도구 세트 + 확장) |
| CLI 인자 파서 | clap으로 대체 |
