#!/usr/bin/env bash
# vendiOS ISO build script
# Usage: sudo ./build.sh [--clean] [output-dir]
#   --clean  wipe work dir and do a full rebuild (slow)
#   default  incremental rebuild (fast, reuses cached package install)

set -euo pipefail

PROFILE="$(cd "$(dirname "$0")/archiso" && pwd)"
WORK="/home/vendi/vendiOS/work"
CLEAN=false
OUT=""

for arg in "$@"; do
    case "$arg" in
        --clean) CLEAN=true ;;
        *)       OUT="$arg" ;;
    esac
done
OUT="${OUT:-$(pwd)/out}"

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root (sudo ./build.sh)"
    exit 1
fi

if ! command -v mkarchiso &>/dev/null; then
    echo "error: archiso not installed — install it first:"
    echo "  sudo pacman -S archiso"
    exit 1
fi

echo ""
echo "  vendiOS ISO builder"
echo "  profile : ${PROFILE}"
echo "  output  : ${OUT}"
echo "  work    : ${WORK}"
echo "  mode    : $( $CLEAN && echo 'full (--clean)' || echo 'incremental' )"
echo ""

mkdir -p "$OUT"

# ── Build Rust binaries (compositor + CLI + demo) ─────────────────────────────
# These don't come from packages.x86_64 — they're vendored into the airootfs
# overlay so mkarchiso picks them up. Built as the invoking user so cargo can
# use their ~/.cargo cache; copied into archiso/airootfs/usr/bin/ before the
# squashfs is built.
RUST_SRC="$(cd "$(dirname "$0")/src" && pwd)"
RUST_BIN_DIR="${PROFILE}/airootfs/usr/bin"
BUILD_USER="${SUDO_USER:-$(whoami)}"

echo "  Building Rust workspace as user '${BUILD_USER}'..."
if [[ "$BUILD_USER" != "root" ]]; then
    sudo -u "$BUILD_USER" -H bash -lc \
        "cd '${RUST_SRC}' && cargo build --release --locked -p vendiwm -p vendi-ctl -p vendi-demo"
else
    (cd "$RUST_SRC" && cargo build --release --locked -p vendiwm -p vendi-ctl -p vendi-demo)
fi

for bin in vendiwm vendi-ctl vendi-demo; do
    src="${RUST_SRC}/target/release/${bin}"
    if [[ ! -x "$src" ]]; then
        echo "error: built binary missing: ${src}"
        exit 1
    fi
    install -Dm755 "$src" "${RUST_BIN_DIR}/${bin}"
done
echo "  Rust binaries staged into airootfs."


if $CLEAN; then
    # unmount any stale bind-mounts from a previous interrupted build
    for mnt in proc sys dev dev/pts run; do
        mountpoint -q "${WORK}/x86_64/airootfs/${mnt}" 2>/dev/null && \
            umount -lf "${WORK}/x86_64/airootfs/${mnt}" 2>/dev/null || true
    done
    [[ -d "$WORK" ]] && rm -rf "$WORK"
else
    # incremental: drop only the steps that depend on airootfs content
    # so package installation is reused but customization reruns
    for marker in \
        base._make_custom_airootfs \
        base._make_customize_airootfs \
        base._cleanup_pacstrap_dir \
        base._check_if_initramfs_has_ucode \
        base._make_pkglist \
        base._make_version \
        base._make_boot_on_iso9660 \
        base._make_bootmode_bios.syslinux \
        base._make_bootmode_uefi.systemd-boot \
        base._make_boot_on_fat \
        base._make_common_grubenv_and_loopbackcfg \
        base._prepare_airootfs_image \
        base._mkairootfs_squashfs
    do
        rm -f "${WORK}/${marker}"
    done
    # also restore airootfs from a clean state by re-copying profile files
    # (mkarchiso will overlay profile airootfs on top of the installed packages)
fi

mkarchiso \
    -v \
    -w "$WORK" \
    -o "$OUT" \
    "$PROFILE"

# ── Replace syslinux+systemd-boot with Limine ─────────────────────────────────
# Live ISO uses Limine (same as installed system) for consistency.
# Boots silently into vendi-install — no menu.

ISO_DIR="${WORK}/iso"
ISO_PATH="$(ls -1t "${OUT}"/*.iso 2>/dev/null | head -1)"
ISO_LABEL="VENDIOS_$(date +%Y%m)"

if [[ -z "$ISO_PATH" || ! -d "$ISO_DIR" ]]; then
    echo "error: mkarchiso did not produce expected outputs"
    exit 1
fi

echo ""
echo "  Switching bootloader to Limine..."

# strip mkarchiso's bootloaders
rm -rf "${ISO_DIR}/boot/syslinux" "${ISO_DIR}/boot/grub" \
       "${ISO_DIR}/EFI" "${ISO_DIR}/loader"

# install limine boot files
mkdir -p "${ISO_DIR}/boot/limine" "${ISO_DIR}/EFI/BOOT"
install -m 0644 /usr/share/limine/limine-bios.sys      "${ISO_DIR}/boot/limine/"
install -m 0644 /usr/share/limine/limine-bios-cd.bin   "${ISO_DIR}/boot/limine/"
install -m 0644 /usr/share/limine/limine-uefi-cd.bin   "${ISO_DIR}/boot/limine/"
install -m 0644 /usr/share/limine/BOOTX64.EFI          "${ISO_DIR}/EFI/BOOT/"
install -m 0644 /usr/share/limine/BOOTIA32.EFI         "${ISO_DIR}/EFI/BOOT/" 2>/dev/null || true

# kernel cmdline — matches the silent boot flags
CMDLINE="archisobasedir=arch archisolabel=${ISO_LABEL} quiet loglevel=0"
CMDLINE+=" rd.systemd.show_status=false rd.udev.log_level=3"
CMDLINE+=" systemd.show_status=false vt.global_cursor_default=0"

# limine.conf — single entry, instant boot (no menu)
{
    echo "timeout: 0"
    echo "serial: no"
    echo ""
    echo "/vendiOS Live"
    echo "    protocol: linux"
    echo "    kernel_path: boot():/arch/boot/x86_64/vmlinuz-linux"
    echo "    cmdline: ${CMDLINE}"
    echo "    module_path: boot():/arch/boot/x86_64/initramfs-linux.img"
    for ucode in amd-ucode.img intel-ucode.img; do
        [[ -f "${ISO_DIR}/arch/boot/x86_64/${ucode}" ]] && \
            echo "    module_path: boot():/arch/boot/x86_64/${ucode}"
    done
} > "${ISO_DIR}/limine.conf"

# rebuild ISO with limine BIOS+UEFI hybrid boot
rm -f "$ISO_PATH"
xorriso -as mkisofs \
    -iso-level 3 \
    -full-iso9660-filenames \
    -joliet -joliet-long -rational-rock \
    -volid "$ISO_LABEL" \
    -appid "vendiOS" \
    -publisher "vendiOS" \
    -preparer "vendiOS Build" \
    -b boot/limine/limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    -hide boot.catalog \
    --efi-boot boot/limine/limine-uefi-cd.bin \
    -efi-boot-part --efi-boot-image \
    --protective-msdos-label \
    -output "$ISO_PATH" \
    "$ISO_DIR" 2>&1 | grep -vE '^xorriso :' || true

# write limine BIOS stage1 to the ISO
limine bios-install "$ISO_PATH" >/dev/null

echo ""
echo "  ISO built:"
ls -lh "${OUT}"/*.iso 2>/dev/null || echo "  (check output above for errors)"
echo ""
