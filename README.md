# terminal-windows

터미널에서 실제 Windows 가상 머신을 만들고, 부팅하고, 보고, 조작합니다. Codex·Claude Code·Cursor 같은 MCP 에이전트가 **호스트가 아닌 격리된 Windows 게스트**에서 GUI 작업을 하도록 설계되었습니다. [govijr/terminal-mac](https://github.com/govijr/terminal-mac)의 구조와 개념을 기반으로 합니다.

> **상태: 초기 개발 (기초 작업 단계).** 무엇이 실측 검증되었는지는 [docs/compatibility.md](docs/compatibility.md)를 보세요. 실제 Windows VM 부팅은 아직 검증되지 않았습니다.

## 설계 요점

- **QEMU** 기반 (macOS: HVF, Windows: WHPX), Windows 호스트용 **Hyper-V** 백엔드 계획.
- **이중 제어면**: 하이퍼바이저 콘솔(QMP)로 설치·로그인·UAC까지 조작, 게스트 에이전트로 유니코드 입력·UI Automation.
- 네트워크 리스너 없음, 공유폴더/클립보드 공유 없음, Windows 요구사항 우회 없음.

자세한 기획과 원본 프롬프트의 문제점 분석: [docs/PLAN.md](docs/PLAN.md)

## 요구사항

- Rust 1.89+
- QEMU 7.1+ 와 swtpm (macOS: `brew install qemu swtpm`)
- Windows 11 설치 ISO: `terminal-windows image download`가 Microsoft에서 받아 검증합니다(아래). Apple Silicon은 **ARM64**.

## 설치

```sh
curl -fsSL https://github.com/dlfkdLR/terminal-windows/releases/latest/download/get.sh | bash
```

최신 릴리스를 받아 `SHA256SUMS`로 검증한 뒤 `~/.local/bin/terminal-windows` 하나만 설치합니다(관리자 권한·셸 설정 변경 없음). Windows: `irm https://github.com/dlfkdLR/terminal-windows/releases/latest/download/install.ps1 | iex` (실험적).

### Apple Silicon: Secure Boot 펌웨어

Windows 11은 Secure Boot 가능 펌웨어를 요구하는데 Homebrew QEMU의 펌웨어에는 없습니다. 한 번만 설치하세요(릴리스에서 받아 SHA-256 검증):

```sh
terminal-windows firmware install
terminal-windows firmware list      # Secure Boot 지원 여부 확인
```

경위와 직접 빌드 방법: [firmware/README.md](firmware/README.md)

## 빌드와 사용

```sh
cargo build --release
B=target.nosync/release/terminal-windows   # .cargo/config.toml: 동기화 폴더 대비

$B doctor                                   # QEMU/가속기/펌웨어/TPM 실측
$B image download --language Korean         # Microsoft에서 받기 + 공식 SHA-256 검증
$B firmware install                         # (Apple Silicon, 최초 1회) Secure Boot 펌웨어
$B create dev                               # 캐시의 검증된 ISO 자동 사용 (--iso로 직접 지정 가능)
$B view dev                                 # 터미널에서 화면 보기·조작 (Ghostty/Kitty/WezTerm), Ctrl+] 종료
$B start dev
$B action dev screenshot                    # PNG 경로 출력
$B action dev click 400 300
$B action dev key ctrl+shift+esc
$B action dev type "Hello Windows"          # 에이전트 없이는 US-ASCII만
$B stop dev                                 # ACPI 종료 요청 (--force: 즉시 전원 차단)
$B delete dev                               # 확인 필요 (--force로 생략)
```

### Windows ISO 받기

terminal-mac이 Apple에서 macOS 이미지를 받듯, `image download`는 **Microsoft 공식 다운로드 페이지**에서 현재 릴리스와 언어별 SHA-256을 읽고, 페이지가 쓰는 것과 같은 API로 다운로드 링크를 요청합니다.

- 링크가 발급되면: 이어받기 가능한 다운로드 → SHA-256 검증 → 캐시 등록.
- Microsoft가 자동 요청을 거부하면(봇 차단 "Sentinel", 2026-11-05 실측 시 항상 거부): **우회하지 않고** 공식 페이지를 브라우저로 열어 안내하고, 다운로드 폴더를 감시하다가 완료되면 SHA-256을 검증해 캐시로 옮깁니다.
- 이미 받은 ISO: `image import PATH [--move]` (공식 해시와 일치해야 등록).
- `image info`: 다운로드 없이 현재 버전·해시 확인. `image list`: 캐시 목록.

ISO를 번들하거나 재배포하지 않으며, 검증되지 않은 파일은 캐시에 넣지 않습니다(`create --iso`로 명시하면 사용 가능).

모든 명령은 `--json`을 지원하며 실패 시 안정적인 오류 코드와 종료 코드를 반환합니다(`tw-core/src/error.rs`).

## 에이전트 연결 (MCP)

```sh
claude mcp add terminal-windows -- /absolute/path/to/terminal-windows mcp dev
terminal-windows mcp dev --config     # 다른 MCP 클라이언트용 JSON 스니펫
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.terminal-windows]
command = "/absolute/path/to/terminal-windows"
args = ["mcp", "dev"]
```

도구: `screenshot, click, move, drag, scroll, type, key, status, list_vms, start_vm, stop_vm`. 좌표는 스크린샷 픽셀(좌상단 원점)입니다. 에이전트는 observe → act → verify 순서로 사용하세요(`screenshot: true` 옵션).

## 개발

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

문서: [architecture](docs/architecture.md) · [protocol](docs/protocol.md) · [security](docs/security.md) · [compatibility](docs/compatibility.md) · [upstream 분석](docs/upstream-analysis.md) · [코드 출처](docs/PROVENANCE.md)

## 라이선스

MIT. upstream terminal-mac(MIT, Copyright (c) 2026 Govijr)에서 개작한 부분은 [docs/PROVENANCE.md](docs/PROVENANCE.md)와 각 파일 머리말에 표기되어 있습니다.
