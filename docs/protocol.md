# 호스트↔게스트 프로토콜 v1

구현: `crates/tw-protocol`. 게스트 측 사용: `guest/windows-agent`.

## 전송

| 백엔드 | 호스트 엔드포인트 | 게스트 장치 |
|---|---|---|
| QEMU (macOS/Linux 호스트) | Unix 소켓 `<runtime>/agent.sock` (디렉터리 0700) | virtio-serial 포트 `\\.\Global\com.terminal-windows.agent.0` |
| QEMU (Windows 호스트) | named pipe `\\.\pipe\tw-<tag>-agent` | 동일 |
| Hyper-V (계획) | hvsocket (서비스 GUID) | AF_HYPERV 소켓 |

어느 경우에도 TCP/IP 리스너가 없습니다. 프로토콜은 순서가 보장되는 바이트 스트림만 가정합니다.

## 프레임

```text
u32 BE  len        (= 8 + 32 + payload 길이)
u64 BE  seq        (핸드셰이크 0, 이후 방향별 1부터 1씩 증가)
[32]    mac        HMAC-SHA256(session_key, dir || seq || payload), 핸드셰이크는 0
[...]   payload    UTF-8 JSON (Message)
```

- `dir`: 호스트→게스트 `0x01`, 게스트→호스트 `0x02` (반사 공격 방지)
- 크기 제한(할당 전 검사): 게스트가 받는 프레임 128 KiB, 호스트가 받는 프레임 16 MiB, 핸드셰이크 16 KiB
- MAC 검증 → 시퀀스 검증 순서. 실패 시 연결을 폐기하고 재연결(복구는 상위 계층)

## 핸드셰이크 (상호 인증)

`K` = VM별 256비트 키 (`<vm>/agent.key`, 0600; 게스트에는 tools ISO로 전달).

```text
H → G  hello      { protocol: "terminal-windows", versions: [1], nonce: Nh }
G → H  hello_ack  { version: 1, nonce: Ng, proof: HMAC(K, "tw-v1-ack"  || Nh || Ng), agent: {version, os, arch, capabilities} }
H → G  ready      { proof: HMAC(K, "tw-v1-ready" || Ng || Nh) }
session_key = HMAC(K, "tw-v1-session" || Nh || Ng)
```

- 공통 버전이 없으면 게스트는 `goodbye`를 보내고 종료, 호스트는 `VersionMismatch` 보고
- 각 방향 증명은 상수 시간 비교(`hmac::verify_slice`)
- 핸드셰이크 전체에 타임아웃 적용

## 메시지

```jsonc
{"type":"request","id":7,"timeout_ms":5000,"action":"click","x":10,"y":20,"button":"right","count":2}
{"type":"response","id":7,"status":"ok","result":{"kind":"done"}}
{"type":"response","id":8,"status":"error","code":"secure_desktop","message":"..."}
{"type":"event","event":"session_changed","session":"locked"}
{"type":"goodbye","reason":"..."}
```

액션: `status, screenshot{max_width,max_height}, move, click{button,count}, drag{to_x,to_y}, scroll{dx,dy}, type{text}, key{chord}, ui_tree{max_depth,max_nodes}, shutdown`. 알 수 없는 액션(예: `exec`)은 역직렬화 단계에서 거부됩니다 — **임의 명령 실행 액션은 프로토콜에 존재하지 않습니다.**

오류 코드: `invalid_argument, unsupported, secure_desktop, elevated_target, no_interactive_session, timeout, internal`.

## 타임아웃

절대 시각 대신 **상대 `timeout_ms`** 를 씁니다(upstream의 절대 deadline은 게스트 시계 어긋남에 취약). 호스트는 `timeout + 250ms` 안에 응답이 없으면 실패 처리합니다. v1은 연결당 동시 요청 1개.

## 좌표

모든 좌표는 게스트 주 모니터의 **물리 픽셀**(좌상단 원점). 에이전트는 Per-Monitor-V2 DPI aware이며 `SendInput` 절대좌표(0–65535)로 변환합니다(`tw_core::coords::normalize`).

## 보안상 한계 (정직한 기술)

virtio-serial 포트는 게스트 내부의 어떤 프로세스도 열 수 있으므로, 게스트 악성코드가 키를 훔치면 에이전트를 사칭할 수 있습니다. 따라서 Plane G가 보고한 값은 `guest-reported`로 표시하고, 화면의 기준 진실은 하이퍼바이저 캡처(Plane H)입니다. 인증의 목적은 VM 혼동 방지, 호스트 측 다른 로컬 프로세스의 사칭 방지, 재생/순서/반사 공격 방지입니다.

## 테스트

`cargo test -p tw-protocol`: 왕복, 잘못된 키(양측 실패), 무응답 타임아웃, 버전 불일치, 위조 MAC, 재생, 반사, 크기 초과(할당 전 거부), 알 수 없는 액션 거부.
