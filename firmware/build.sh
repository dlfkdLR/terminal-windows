#!/usr/bin/env bash
# Builds Secure Boot capable UEFI firmware for the QEMU "virt" machine (AArch64).
#
# Why: Windows 11 Setup refuses to continue unless the firmware supports Secure Boot.
# The firmware shipped by Homebrew's QEMU has none, and the Debian/UTM builds we
# tried either hang under Apple's Hypervisor.framework (Debian) or lack ramfb (UTM).
# This script rebuilds the very same edk2 revision that QEMU ships, with the same
# options as QEMU's roms/edk2-build.config, plus SECURE_BOOT_ENABLE=TRUE.
#
# Requirements (macOS: brew install llvm lld acpica nasm): clang, ld.lld, llvm-ar,
# llvm-objcopy, iasl, nasm, python3, make.
#
# Usage: firmware/build.sh OUT_DIR
# Output: OUT_DIR/code.fd and OUT_DIR/vars.fd (64 MiB each, ready for pflash).
set -euo pipefail

QEMU_VERSION="${QEMU_VERSION:-11.1.3}"
QEMU_SHA256="${QEMU_SHA256:-5a8289c7940dc77d3873c81a6c29ba3afe1446fc9d36e77fa1658cdbecb84b22}"
OUT="$(cd "$(dirname "${1:?usage: build.sh OUT_DIR}")" && pwd)/$(basename "$1")"
mkdir -p "$OUT"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

sha256() { if command -v shasum >/dev/null; then shasum -a 256 "$1" | cut -c1-64; else sha256sum "$1" | cut -c1-64; fi; }

echo "==> downloading QEMU $QEMU_VERSION sources (bundles the matching edk2)"
curl -fsSL "https://download.qemu.org/qemu-$QEMU_VERSION.tar.xz" -o "$WORK/qemu.tar.xz"
[ "$(sha256 "$WORK/qemu.tar.xz")" = "$QEMU_SHA256" ] || { echo "SHA-256 mismatch for the QEMU tarball" >&2; exit 1; }
tar -xf "$WORK/qemu.tar.xz" -C "$WORK" "qemu-$QEMU_VERSION/roms/edk2"
EDK2="$WORK/qemu-$QEMU_VERSION/roms/edk2"

echo "==> toolchain"
LLVM="$WORK/llvm"; mkdir -p "$LLVM"
LLVM_PREFIX="$(brew --prefix llvm 2>/dev/null || echo /usr)"
for t in clang clang++ llvm-ar llvm-objcopy llvm-objdump llvm-nm llvm-ranlib; do
  src="$LLVM_PREFIX/bin/$t"; [ -x "$src" ] || src="$(command -v "$t")"; ln -sf "$src" "$LLVM/$t"
done
ln -sf "$(command -v ld.lld)" "$LLVM/ld.lld"

# Newer Apple/LLVM clang rejects a harmless macro redefinition in the *host-side*
# BaseTools under -Werror. This only affects the build tools, not the firmware.
sed -i.bak 's/ -Werror//g' "$EDK2/BaseTools/Source/C/Makefiles/header.makefile"

export PATH="$LLVM:$PATH" CLANGDWARF_BIN="$LLVM/" WORKSPACE="$EDK2" PACKAGES_PATH="$EDK2" PYTHON_COMMAND=python3
cd "$EDK2"
echo "==> BaseTools"
make -C BaseTools -j8 >"$WORK/basetools.log" 2>&1 || { tail -30 "$WORK/basetools.log"; exit 11; }
set +u; . ./edksetup.sh BaseTools >/dev/null; set -u

echo "==> ArmVirtQemu (AARCH64, RELEASE, Secure Boot)"
build -a AARCH64 -t CLANGDWARF -b RELEASE -p ArmVirtPkg/ArmVirtQemu.dsc -n 8 \
  -D NETWORK_HTTP_BOOT_ENABLE=TRUE -D NETWORK_IP6_ENABLE=TRUE -D NETWORK_TLS_ENABLE=TRUE \
  -D NETWORK_ISCSI_ENABLE=TRUE -D NETWORK_ALLOW_HTTP_CONNECTIONS=TRUE \
  -D TPM2_ENABLE=TRUE -D TPM2_CONFIG_ENABLE=TRUE -D TPM1_ENABLE=TRUE -D CAVIUM_ERRATUM_27456=TRUE \
  -D SECURE_BOOT_ENABLE=TRUE -D DEBUG_PRINT_ERROR_LEVEL=0x80000000 >"$WORK/build.log" 2>&1 \
  || { tail -40 "$WORK/build.log"; exit 12; }

FV="Build/ArmVirtQemu-AARCH64/RELEASE_CLANGDWARF/FV"
cp "$FV/QEMU_EFI.fd"  "$OUT/code.fd"
cp "$FV/QEMU_VARS.fd" "$OUT/vars.fd"
# pflash devices need exactly 64 MiB.
truncate -s 64m "$OUT/code.fd" "$OUT/vars.fd" 2>/dev/null || { dd if=/dev/zero of="$OUT/code.fd" bs=1 count=0 seek=67108864 2>/dev/null; dd if=/dev/zero of="$OUT/vars.fd" bs=1 count=0 seek=67108864 2>/dev/null; }
echo "==> done"; ls -l "$OUT"
