# UEFI firmware for Windows 11 on AArch64

Windows 11 Setup stops with **"PC must support Secure Boot"** unless the UEFI
firmware is Secure Boot capable. QEMU for AArch64 has no such firmware in the
common distributions, so this directory contains the build that terminal-windows
uses. `terminal-windows firmware install` downloads the published result and
verifies its SHA-256.

## What was tried (Apple Silicon, QEMU 11.1.3, HVF, 2026-11-05)

| Firmware | Result |
|---|---|
| Homebrew QEMU `edk2-aarch64-code.fd` | boots under HVF, **no Secure Boot** → Setup refuses |
| Debian `qemu-efi-aarch64` 2025.02 and 2026.08 (`AAVMF_CODE.secboot.fd`) | **hangs at "Start boot option" under HVF** (100 % of one vCPU), runs under TCG; no machine option changed that (highmem, gic-version, smp) |
| UTM 5.0.8 `edk2-aarch64-secure-code.fd` | runs under HVF but has **no ramfb**; with virtio-gpu, Windows has no display driver and the screen freezes |
| **This build** (edk2 from QEMU 11.1.3 sources + `SECURE_BOOT_ENABLE=TRUE`) | boots Windows Setup under HVF and passes the Secure Boot requirement |

## What the build is

`build.sh` downloads the QEMU 11.1.3 source tarball (pinned SHA-256), which
bundles the exact edk2 revision of Homebrew's firmware, and builds
`ArmVirtPkg/ArmVirtQemu.dsc` for AArch64 with the options from QEMU's own
`roms/edk2-build.config`, plus `SECURE_BOOT_ENABLE=TRUE`. Nothing else is changed.

* No Secure Boot keys are enrolled (the firmware starts in *setup mode*). Windows
  only requires the platform to be *capable* of Secure Boot; it does not need it
  turned on. Enrolling keys is possible later.
* Licenses: edk2 is BSD-2-Clause-Patent, and it embeds OpenSSL (Apache-2.0) and
  other components under their own licenses; see the `License*.txt` files in the
  edk2 sources. Releases publish the binaries together with this build script.

## Building yourself

```sh
brew install llvm lld acpica nasm
firmware/build.sh ./out          # → out/code.fd, out/vars.fd
terminal-windows firmware install --from ./out
```
