#!/usr/bin/env bash
# vendiOS ISO build script
# Usage: sudo ./build.sh [--clean] [output-dir]
#   --clean  wipe work dir and do a full rebuild (slow)
#   default  incremental rebuild (fast, reuses cached package install)

set -euo pipefail

PROFILE="$(cd "$(dirname "$0")/archiso" && pwd)"
WORK="/home/vendi/vendiOS/work"
CLEAN=false
OFFLINE=false
OUT=""

for arg in "$@"; do
    case "$arg" in
        --clean)   CLEAN=true ;;
        --offline) OFFLINE=true ;;
        *)         OUT="$arg" ;;
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
# vendi-chatd (vendiMessage backend) needs its real Matrix backend, so it builds
# with --features matrix in a second pass.
if [[ "$BUILD_USER" != "root" ]]; then
    sudo -u "$BUILD_USER" -H bash -lc \
        "cd '${RUST_SRC}' && cargo build --release --locked -p vendiwm -p vendi-ctl -p vendi-demo -p vendibar -p vendi-menu && cargo build --release --locked --features matrix -p vendi-chatd"
else
    (cd "$RUST_SRC" && cargo build --release --locked -p vendiwm -p vendi-ctl -p vendi-demo -p vendibar -p vendi-menu && cargo build --release --locked --features matrix -p vendi-chatd)
fi

for bin in vendiwm vendi-ctl vendi-demo vendibar vendi-menu vendi-chatd; do
    src="${RUST_SRC}/target/release/${bin}"
    if [[ ! -x "$src" ]]; then
        echo "error: built binary missing: ${src}"
        exit 1
    fi
    install -Dm755 "$src" "${RUST_BIN_DIR}/${bin}"
done
echo "  Rust binaries staged into airootfs."

# Keep the shipped quickshell bar config in sync with its canonical source
# (src/vendibar-pro). They are separate trees and drift silently otherwise —
# which once shipped a stale, half-transparent bar. Always re-sync here.
if [[ -d "${RUST_SRC}/vendibar-pro" ]]; then
    mkdir -p "${PROFILE}/airootfs/etc/xdg/quickshell/vendibar-pro"
    cp -a "${RUST_SRC}/vendibar-pro/." \
          "${PROFILE}/airootfs/etc/xdg/quickshell/vendibar-pro/"
    echo "  Synced src/vendibar-pro -> airootfs quickshell config."
fi

# ── Offline package repo ──────────────────────────────────────────────────────
# Bundle a complete local pacman repo into the ISO so the installer can pacstrap
# the whole target system (base + desktop + apps + every-vendor GPU driver) with
# NO network. The installer auto-detects /opt/vendios/repo and switches to it.
OFFLINE_DIR="${PROFILE}/airootfs/opt/vendios"
REPO="${OFFLINE_DIR}/repo"
if $OFFLINE && [[ -f "${REPO}/vendios.db.tar.zst" ]]; then
    echo "  Reusing existing offline repo ($(ls "${REPO}"/*.pkg.tar.zst 2>/dev/null | wc -l) packages) — delete ${REPO} to force a fresh download."
elif $OFFLINE; then
    echo "  Bundling offline package repo (downloads the full target package set)..."
    # BASE_PKGS lives in system.sh; sourcing it only defines the array.
    # shellcheck source=/dev/null
    source "${PROFILE}/airootfs/usr/lib/vendi/system.sh"
    # All-vendor GPU drivers (sys_graphics installs the matching ones offline),
    # plus archlinux-keyring so the target keyring can be populated for later
    # online updates, plus btrfs-progs (added as an `extra` at install time).
    OFFLINE_PKGS=(
        "${BASE_PKGS[@]}"
        vulkan-radeon vulkan-intel intel-media-driver
        nvidia-open-dkms nvidia-utils libva-nvidia-driver egl-wayland
        archlinux-keyring btrfs-progs
    )
    rm -rf "$OFFLINE_DIR"
    mkdir -p "$REPO"
    tmpdb="$(mktemp -d)"
    # pacman 7 drops to the unprivileged DownloadUser (alpm) for downloads,
    # which can't write our root-owned tmp dbpath / repo dir. Use a config with
    # that line stripped so downloads run as root and can populate the repo.
    buildconf="$(mktemp)"
    grep -viE '^[[:space:]]*DownloadUser' /etc/pacman.conf > "$buildconf"
    # -Syw with a throwaway dbpath downloads every package + full dependency
    # tree into the repo dir without touching the host's pacman state.
    pacman -Syw --noconfirm --config "$buildconf" --dbpath "$tmpdb" --cachedir "$REPO" "${OFFLINE_PKGS[@]}"
    rm -rf "$tmpdb" "$buildconf"
    repo-add "${REPO}/vendios.db.tar.zst" "${REPO}"/*.pkg.tar.zst >/dev/null
    cat > "${OFFLINE_DIR}/offline-pacman.conf" <<'EOF'
# vendiOS offline installer repo — used by vendi-install when the bundled
# package repo is present (no network, no signature/keyring requirement).
[options]
HoldPkg      = pacman glibc
Architecture = auto
SigLevel          = Never
LocalFileSigLevel = Optional

[vendios]
SigLevel = Never
Server   = file:///opt/vendios/repo
EOF
    echo "  Offline repo staged: $(ls "${REPO}"/*.pkg.tar.zst 2>/dev/null | wc -l) packages, $(du -sh "$REPO" | cut -f1)."
else
    # Online build — make sure no stale offline repo is left in the overlay.
    rm -rf "$OFFLINE_DIR"
fi


if $CLEAN; then
    # unmount any stale bind-mounts from a previous interrupted build
    for mnt in proc sys dev dev/pts run; do
        mountpoint -q "${WORK}/x86_64/airootfs/${mnt}" 2>/dev/null && \
            umount -lf "${WORK}/x86_64/airootfs/${mnt}" 2>/dev/null || true
    done
    [[ -d "$WORK" ]] && rm -rf "$WORK"
    # Also wipe the cached offline repo — its package set must match BASE_PKGS,
    # which may have changed since the last build. --offline will re-download.
    rm -rf "${PROFILE}/airootfs/opt/vendios"
else
    # Incremental: reuse the expensive pacstrap of the live root, but force
    # EVERYTHING downstream of it to rebuild — the profile airootfs overlay,
    # customize, squashfs and the ISO image. mkarchiso's stage-marker names
    # vary by version, so rather than guess them, delete every stage marker
    # except the three pacstrap/work-dir ones. (The previous hard-coded list
    # named markers that didn't exist, so the airootfs overlay never re-copied
    # and stale content shipped.)
    if [[ -d "$WORK" ]]; then
        find "$WORK" -maxdepth 1 -type f -name '*._*' \
            ! -name 'base._make_packages' \
            ! -name 'base._make_pacman_conf' \
            ! -name 'base._make_work_dir' \
            -delete
        # The overlay is rsync'd onto the pacstrapped root; wipe the previous
        # overlay's vendi files so renamed/removed files don't linger.
        rm -rf "${WORK}/x86_64/airootfs/opt/vendios" 2>/dev/null || true
    fi
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
