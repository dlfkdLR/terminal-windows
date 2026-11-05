# 코드 출처 기록 (Provenance)

upstream: [govijr/terminal-mac](https://github.com/govijr/terminal-mac) @ `ac787d577de58a3804d32c34b73af453e83f77d8`, MIT License, Copyright (c) 2026 Govijr. 원문 라이선스: [UPSTREAM-LICENSE-terminal-mac.txt](UPSTREAM-LICENSE-terminal-mac.txt).

Swift 소스를 그대로 복사한 파일은 없습니다. 아래는 **알고리즘/동작을 Rust로 개작(adapted)** 한 항목이며, 해당 파일 상단 주석에도 출처와 저작권을 표기했습니다.

| terminal-windows 파일 | upstream 원본 | 구분 | 변경 사항 |
|---|---|---|---|
| `crates/tw-input/src/terminal.rs` | `Sources/TerminalMacCore/TerminalInput.swift` | 개작 | SS3 키, CSI 수정자(`ESC[1;5A`), `ESC[n~` 기능키 추가. upstream 테스트 2개 이식 |
| `crates/tw-renderer/src/layout.rs` | `Sources/TerminalUI/TerminalMetrics.swift` (`ImageLayout`) | 개작 | 로직 동일, 0 크기 방어 |
| `crates/tw-renderer/src/metrics.rs` | 같은 파일 (`TerminalMetricFilter`) | 개작 | DA1(Sixel 판정) 추가, iTerm2 `ReportCellSize`/OSC 11 제외 |
| `crates/tw-renderer/src/kitty.rs` | `Sources/TerminalMac/DesktopRenderer.swift` | 개념 차용 | 이중 버퍼 이미지 id(2/3), z-index, 4096바이트 청크. 공유메모리 경로는 미이식 |
| `crates/tw-core/src/name.rs` | `Models.swift` `validateName` | 개작 | Windows 예약 장치명(CON, NUL…) 거부 추가 |
| `crates/tw-mcp/src/server.rs` | `Sources/TerminalMac/AgentServer.swift` | 개념 차용 | 도구 이름/설명 문구 일부, observe→act→verify 지침. 엄격 스키마 검증·버전 협상·structuredContent는 신규 |
| `crates/tw-core/src/store.rs` | `Models.swift` `Store`, `Setup.swift` `FileLock` | 개념 차용 | 원자적 저장, 배타 잠금, 부분 생성물 보존 원칙 |

그 외 모든 코드(QEMU 백엔드, QMP, 펌웨어 디스크립터, 프로토콜 v1, 게스트 에이전트, CLI)는 신규 작성입니다.
