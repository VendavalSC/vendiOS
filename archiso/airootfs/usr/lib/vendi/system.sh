#!/usr/bin/env bash
# vendiOS system configuration library

chroot_run() { arch-chroot /mnt "$@"; }

BASE_PKGS=(
    base base-devel linux linux-headers linux-firmware
    amd-ucode intel-ucode
    networkmanager wpa_supplicant iwd
    sudo vim nano git curl wget
    fastfetch htop
    man-db bash-completion zsh
    terminus-font ttf-jetbrains-mono-nerd ttf-nerd-fonts-symbols-common
    noto-fonts noto-fonts-emoji
    limine efibootmgr
    btrfs-progs e2fsprogs dosfstools
    openssh reflector
    pipewire pipewire-audio pipewire-alsa pipewire-pulse wireplumber
    hyprland xdg-desktop-portal-hyprland xdg-desktop-portal-gtk
    waybar wofi mako foot alacritty quickshell
    brightnessctl playerctl grim slurp wl-clipboard swaylock
    polkit-kde-agent qt5-wayland qt6-wayland
    gtk3 gtk4 gtk4-layer-shell
    # default browser
    firefox
    # default apps — a matched GTK4 set so every common file type opens in
    # something that looks like it belongs (files / images / PDF / text /
    # archives / media). Associations wired in /etc/xdg/mimeapps.list.
    nautilus loupe papers gnome-text-editor file-roller mpv
    # silent boot + LUKS + invisible login manager
    plymouth cryptsetup greetd
    # vendiwm runtime deps (most overlap with hyprland; explicit for safety).
    # mesa = GL for AMD/Intel (the GBM path vendiwm renders through); the
    # vulkan loader + per-vendor drivers are added by sys_graphics at install.
    seatd libinput libxkbcommon mesa vulkan-icd-loader pciutils
    # automated snapshots + recovery
    snapper snap-pac
    # CLI toolkit (improves the bare shell experience)
    eza bat ripgrep fd fzf
    # hardware (python-gobject: powerprofilesctl is a python-gi script)
    bluez bluez-utils power-profiles-daemon python-gobject
    # fingerprint reader support (enroll: `vendi fingerprint`; unlocks lock + sudo)
    fprintd
    # auto-mount removable media
    udisks2 gvfs
    # firewall + time sync
    ufw chrony
    # in-memory swap (no on-disk swap exposure with LUKS) + disk health
    zram-generator smartmontools
)

# Path to the bundled offline repo config; set when the ISO carries a repo.
VENDI_OFFLINE_CONF=/opt/vendios/offline-pacman.conf

sys_pacstrap() {
    if [[ -n "${VENDI_OFFLINE:-}" ]]; then
        # Offline: install everything from the bundled file:// repo. No network,
        # no signatures (SigLevel=Never), so skip -K here — the keyring is set
        # up afterwards (sys_keyring_init) for future online updates.
        pacstrap -C "$VENDI_OFFLINE_CONF" -G /mnt "${BASE_PKGS[@]}" "$@"
    else
        # -K initializes a fresh writable pacman keyring inside the target so
        # pacman in the chroot can import new keys. Without it, the live ISO's
        # keyring is copied as-is and pacman fails to add missing keys.
        pacstrap -K /mnt "${BASE_PKGS[@]}" "$@"
    fi
}

# Install extra packages into the target. Offline pulls from the bundled repo
# via the live env's pacman (--root /mnt); online goes through the chroot.
vendi_pkg_install() {
    [[ $# -gt 0 ]] || return 0
    if [[ -n "${VENDI_OFFLINE:-}" ]]; then
        pacman --root /mnt --config "$VENDI_OFFLINE_CONF" \
               --noconfirm --needed -S "$@"
    else
        chroot_run pacman -S --noconfirm --needed "$@"
    fi
}

# Offline installs skip pacstrap's -K, so populate the target keyring now (from
# the just-installed archlinux-keyring) so the user's first online `pacman -Syu`
# works. No network needed — the keys ship in the package.
sys_keyring_init() {
    chroot_run pacman-key --init        >/dev/null 2>&1 || true
    chroot_run pacman-key --populate archlinux >/dev/null 2>&1 || true
}

sys_genfstab() {
    genfstab -U /mnt >> /mnt/etc/fstab
}

sys_locale() {
    local locale=$1
    echo "LANG=${locale}" > /mnt/etc/locale.conf
    echo "${locale} UTF-8" >> /mnt/etc/locale.gen
    chroot_run locale-gen
}

sys_timezone() {
    local tz=$1
    chroot_run ln -sf "/usr/share/zoneinfo/${tz}" /etc/localtime
    chroot_run hwclock --systohc
}

sys_hostname() {
    local name=$1
    echo "$name" > /mnt/etc/hostname
    cat > /mnt/etc/hosts << EOF
127.0.0.1   localhost
::1         localhost
127.0.1.1   ${name}.localdomain ${name}
EOF
}

sys_keymap() {
    local km=$1
    printf 'KEYMAP=%s\nFONT=ter-v18n\n' "$km" > /mnt/etc/vconsole.conf
}

sys_initramfs() {
    # Args: <fs> [encrypted=0|1]
    # kms loads the DRM driver BEFORE plymouth so plymouthd has a display the
    # moment it starts and the splash covers everything after it (otherwise
    # plymouth starts blind and the device-wait / encrypt text prints first).
    # The encrypt hook then prompts via `plymouth ask-for-password`, drawn
    # through the theme rather than the bare console.
    local fs=$1 encrypted=${2:-0}
    local hooks='base udev autodetect microcode modconf kms plymouth keyboard keymap block'
    [[ "$encrypted" -eq 1 ]] && hooks+=' encrypt'
    [[ "$fs" == 'btrfs' ]] && hooks+=' btrfs' || hooks+=' filesystems'
    hooks+=' fsck'
    sed -i "s|^HOOKS=.*|HOOKS=(${hooks})|" /mnt/etc/mkinitcpio.conf
    chroot_run mkinitcpio -P
}

# ── Plymouth theme install ────────────────────────────────────────────────────
sys_install_plymouth() {
    # Copy the bundled vendiOS theme into the target and set as default.
    local theme_src=/usr/share/plymouth/themes/vendios
    local theme_dst=/mnt/usr/share/plymouth/themes/vendios
    mkdir -p "$theme_dst"
    # Copy the whole theme dir (.plymouth, .script, logo.png, any assets).
    cp -a "${theme_src}/." "${theme_dst}/"
    # Just set the default — sys_initramfs runs mkinitcpio -P afterwards.
    chroot_run plymouth-set-default-theme vendios
}

# ── invisible auto-login via greetd (no agetty, no shell flash) ──────────────
sys_install_greetd() {
    # greetd's initial_session launches /usr/bin/vendi-session as the user on
    # vt1. vendi-session picks the actual compositor from /etc/vendi/session.conf
    # so the user can toggle between Hyprland (default) and vendiwm without
    # touching greetd config. Plymouth holds the splash until the compositor's
    # first frame.
    local user=$1
    mkdir -p /mnt/etc/greetd
    cat > /mnt/etc/greetd/config.toml <<EOF
[terminal]
vt = 1
switch = true

[default_session]
command = "/usr/bin/vendi-session"
user = "${user}"

[initial_session]
command = "/usr/bin/vendi-session"
user = "${user}"
EOF

    # Default session pick — vendiwm, the in-house compositor. Set this to
    # "hyprland" and reboot to fall back to the bundled Hyprland session.
    mkdir -p /mnt/etc/vendi
    cat > /mnt/etc/vendi/session.conf <<EOF
# vendi-session config. Set VENDI_SESSION to "vendiwm" for the default
# in-house compositor, or "hyprland" for the bundled fallback.
VENDI_SESSION=vendiwm
EOF

    # Make sure agetty doesn't fight greetd over tty1
    chroot_run systemctl mask getty@tty1.service >/dev/null 2>&1 || true
    chroot_run systemctl enable greetd.service   >/dev/null 2>&1 || true

    # Plymouth holds the GPU as DRM master for the splash. The session runs as
    # the *user*, who cannot quit the root plymouthd — so on real GPUs (Intel
    # i915) plymouthd keeps /dev/dri/cardN and the compositor's session.open
    # fails with EBUSY (ResourceBusy). Quit Plymouth from a ROOT oneshot
    # ordered right before greetd. We mask the stock quit units and let this
    # single, correctly-ordered service own the handoff (brief black gap, but
    # a reliable boot beats a seamless black screen).
    chroot_run systemctl mask plymouth-quit.service      >/dev/null 2>&1 || true
    chroot_run systemctl mask plymouth-quit-wait.service >/dev/null 2>&1 || true
    cat > /mnt/etc/systemd/system/vendi-plymouth-quit.service <<'PQ'
[Unit]
Description=Quit Plymouth and release DRM before the compositor
After=systemd-user-sessions.service
Before=greetd.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=-/usr/bin/plymouth quit
ExecStartPost=/usr/bin/bash -c 'for i in $(seq 1 50); do pgrep -x plymouthd >/dev/null 2>&1 || exit 0; sleep 0.1; done; pkill -x plymouthd 2>/dev/null; exit 0'
[Install]
WantedBy=graphical.target
PQ
    mkdir -p /mnt/etc/systemd/system/greetd.service.d
    cat > /mnt/etc/systemd/system/greetd.service.d/10-after-plymouth.conf <<'GD'
[Unit]
After=vendi-plymouth-quit.service
Wants=vendi-plymouth-quit.service
GD
    chroot_run systemctl enable vendi-plymouth-quit.service >/dev/null 2>&1 || true

    # Suppress any /etc/issue banner that might leak in failure modes
    : > /mnt/etc/issue
}

# ── pre-update snapshots (snapper + snap-pac) ─────────────────────────────────
sys_install_snapper() {
    # snap-pac runs snapper before+after every pacman transaction, so kernel
    # updates / package breakage become rollback-able with `vendi rollback`.
    [[ "$1" != "btrfs" ]] && return 0

    # Snapper refuses to create a config if .snapshots exists. Our @snapshots
    # subvolume IS already mounted there — temporarily detach it so snapper
    # can scaffold, then write its config to point back at /.snapshots.
    chroot_run bash -c '
        set -e
        umount -l /.snapshots 2>/dev/null || true
        rmdir /.snapshots 2>/dev/null || true
        snapper --no-dbus -c root create-config /
        # snapper created /.snapshots as a regular dir — drop it and re-mount
        # the dedicated @snapshots subvolume there.
        umount -l /.snapshots 2>/dev/null || true
        rm -rf /.snapshots
        mkdir -p /.snapshots
        mount /.snapshots
        chmod 750 /.snapshots
    '

    sed -i 's/^TIMELINE_LIMIT_HOURLY=.*/TIMELINE_LIMIT_HOURLY="5"/;
            s/^TIMELINE_LIMIT_DAILY=.*/TIMELINE_LIMIT_DAILY="7"/;
            s/^TIMELINE_LIMIT_WEEKLY=.*/TIMELINE_LIMIT_WEEKLY="2"/;
            s/^TIMELINE_LIMIT_MONTHLY=.*/TIMELINE_LIMIT_MONTHLY="0"/;
            s/^NUMBER_LIMIT=.*/NUMBER_LIMIT="50"/' \
        /mnt/etc/snapper/configs/root 2>/dev/null || true

    chroot_run systemctl enable snapper-timeline.timer snapper-cleanup.timer
}

# ── audio + bluetooth + power-profiles per-user services ──────────────────────
sys_user_services() {
    local user=$1
    chroot_run systemctl enable bluetooth power-profiles-daemon
    # Enable pipewire / wireplumber as user-level services so sound works on
    # first login without manual `systemctl --user start`.
    chroot_run systemctl --global enable pipewire pipewire-pulse wireplumber
    # Persist the user's session services across reboots
    chroot_run loginctl enable-linger "$user" 2>/dev/null || true
}

# ── pacman.conf polish ───────────────────────────────────────────────────────
sys_pacman_polish() {
    # Faster installs (parallel downloads), prettier output, ILoveCandy easter egg.
    sed -i -E \
        -e 's/^#?Color/Color/' \
        -e 's/^#?VerbosePkgLists/VerbosePkgLists/' \
        -e 's/^#?ParallelDownloads.*/ParallelDownloads = 8/' \
        /mnt/etc/pacman.conf
    # ILoveCandy goes after Color
    grep -q '^ILoveCandy' /mnt/etc/pacman.conf || \
        sed -i '/^Color/a ILoveCandy' /mnt/etc/pacman.conf
}

# ── firewall (UFW) ───────────────────────────────────────────────────────────
sys_install_firewall() {
    # Default-deny incoming, allow outgoing — standard desktop posture.
    chroot_run bash -c '
        ufw default deny incoming
        ufw default allow outgoing
        ufw --force enable
    '
    chroot_run systemctl enable ufw
}

# ── time sync (chrony beats timesyncd for desktops) ──────────────────────────
sys_install_time() {
    # Replace systemd-timesyncd with chrony — faster sync, smaller drift.
    chroot_run systemctl disable systemd-timesyncd 2>/dev/null || true
    chroot_run systemctl enable  chronyd
}

# ── ZRAM swap ─────────────────────────────────────────────────────────────────
sys_install_zram() {
    # Compressed in-memory swap. With LUKS-encrypted root + no disk swapfile,
    # zram is the right answer for memory pressure — fast, never touches disk.
    cat > /mnt/etc/systemd/zram-generator.conf <<'EOF'
[zram0]
zram-size = min(ram / 2, 4096)
compression-algorithm = zstd
swap-priority = 100
fs-type = swap
EOF
}

# ── journald + oomd: keep /var lean, handle memory pressure ──────────────────
sys_install_resilience() {
    # Cap journal size so /var doesn't grow without bound.
    mkdir -p /mnt/etc/systemd/journald.conf.d
    cat > /mnt/etc/systemd/journald.conf.d/vendios.conf <<'EOF'
[Journal]
SystemMaxUse=200M
SystemMaxFileSize=20M
MaxRetentionSec=2week
EOF

    # systemd-oomd kicks before the kernel OOM killer — much friendlier.
    chroot_run systemctl enable systemd-oomd
}

# ── mkinitcpio compression: zstd is faster and smaller than gzip ─────────────
sys_initramfs_polish() {
    sed -i -E 's|^#?COMPRESSION=.*|COMPRESSION="zstd"|' /mnt/etc/mkinitcpio.conf
    sed -i -E 's|^#?COMPRESSION_OPTIONS=.*|COMPRESSION_OPTIONS=(-3)|' /mnt/etc/mkinitcpio.conf
}

# ── SMART disk health monitoring ─────────────────────────────────────────────
sys_install_smartd() {
    chroot_run systemctl enable smartd 2>/dev/null || true
}

# ── post-install kernel sanity hook ──────────────────────────────────────────
sys_install_kernel_sync_hook() {
    # After every linux package transaction, verify the kernel + initramfs
    # actually landed on the FAT32 /boot. Catches silent breakage early.
    mkdir -p /mnt/etc/pacman.d/hooks
    cat > /mnt/etc/pacman.d/hooks/95-vendios-kernel-sync.hook <<'EOF'
[Trigger]
Operation = Install
Operation = Upgrade
Type = Path
Target = usr/lib/modules/*/vmlinuz

[Action]
Description = Verifying kernel + initramfs reached /boot...
When = PostTransaction
Exec = /usr/bin/bash -c 'for f in /boot/vmlinuz-linux /boot/initramfs-linux.img; do [ -f "$f" ] || { echo "MISSING: $f"; exit 1; }; done'
EOF
}

sys_sudo() {
    sed -i 's/^# %wheel ALL=(ALL:ALL) ALL/%wheel ALL=(ALL:ALL) ALL/' /mnt/etc/sudoers
}

# Fingerprint reader support. pam_fprintd is added as `sufficient` ABOVE the
# password line so a swipe satisfies auth, but — crucially — when no reader is
# present (or no print is enrolled) it returns ignore/unavailable and falls
# straight through to the password, so this never locks anyone out. Covers
# sudo + the greeter; the lock screen uses its own vendilock-fprint service.
sys_fingerprint() {
    # the locker's dedicated fingerprint-only service
    install -Dm644 /etc/pam.d/vendilock-fprint /mnt/etc/pam.d/vendilock-fprint \
        2>/dev/null || cat > /mnt/etc/pam.d/vendilock-fprint <<'EOF'
auth     sufficient  pam_fprintd.so
auth     required    pam_deny.so
account  required    pam_permit.so
EOF

    # prepend pam_fprintd to sudo + greetd auth stacks (idempotent)
    local svc
    for svc in sudo greetd; do
        local f="/mnt/etc/pam.d/${svc}"
        [[ -f "$f" ]] || continue
        grep -q 'pam_fprintd.so' "$f" && continue
        sed -i '1i auth      sufficient  pam_fprintd.so' "$f"
    done
}

# Detect the GPU(s) and install the matching drivers. AMD + Intel render
# through Mesa's GBM/GLES path that vendiwm already uses, so they just need
# their Vulkan/VA-API drivers. NVIDIA additionally needs the proprietary
# module, early-KMS (modules in the initramfs) and nvidia_drm.modeset=1 so the
# GPU drives both the console/Plymouth and the Wayland GBM surface vendiwm
# scans out on. Runs after limine.conf + mkinitcpio.conf already exist.
sys_graphics() {
    command -v lspci >/dev/null || return 0
    local gpus; gpus=$(lspci -nn 2>/dev/null | grep -iE 'vga|3d controller|display controller')
    local pkgs=() nvidia=0

    # mesa (in BASE_PKGS) already ships the VA-API + VDPAU drivers, so AMD/Intel
    # only need their Vulkan ICD (+ Intel's media driver for hw video decode).
    if grep -qiE 'amd|ati|radeon' <<<"$gpus"; then
        pkgs+=(vulkan-radeon)
    fi
    if grep -qiE 'intel' <<<"$gpus"; then
        pkgs+=(vulkan-intel intel-media-driver)
    fi
    if grep -qiE 'nvidia' <<<"$gpus"; then
        nvidia=1
        # nvidia-open-dkms: the open kernel modules (now the only packaged
        # NVIDIA driver in Arch), built via dkms against the stock `linux`
        # kernel. Supports Turing (GTX 16xx / RTX 20xx) and newer; older cards
        # need a legacy AUR driver. egl-wayland for the EGLStream fallback.
        pkgs+=(nvidia-open-dkms nvidia-utils libva-nvidia-driver egl-wayland)
    fi

    if [[ ${#pkgs[@]} -gt 0 ]]; then
        vendi_pkg_install "${pkgs[@]}" || true
    fi

    if [[ $nvidia -eq 1 ]]; then
        # early-KMS modules in the initramfs
        local mc=/mnt/etc/mkinitcpio.conf
        if ! grep -q 'nvidia_drm' "$mc"; then
            if grep -qE '^MODULES=\(\)' "$mc"; then
                sed -i 's/^MODULES=()/MODULES=(nvidia nvidia_modeset nvidia_uvm nvidia_drm)/' "$mc"
            else
                sed -i 's/^MODULES=(/MODULES=(nvidia nvidia_modeset nvidia_uvm nvidia_drm /' "$mc"
            fi
        fi
        # kernel param so DRM modesetting is on from boot
        grep -q 'nvidia_drm.modeset' /mnt/boot/limine.conf 2>/dev/null || \
            sed -i 's|\(^[[:space:]]*cmdline:.*\)|\1 nvidia_drm.modeset=1|' /mnt/boot/limine.conf
        # rebuild the initramfs against the kernel modules + sync to FAT32 boot
        chroot_run mkinitcpio -P || true
    fi
}

sys_root_password() {
    printf 'root:%s' "$1" | chroot_run chpasswd
}

sys_user_create() {
    local user=$1 pass=$2
    chroot_run useradd -m -G wheel,audio,video,storage,optical,input -s /bin/zsh "$user"
    printf '%s:%s' "$user" "$pass" | chroot_run chpasswd
    # Silence "Last login: ..." motd so the screen stays clean during the
    # tiny window between Plymouth and Hyprland.
    : > "/mnt/home/${user}/.hushlogin"
    chroot_run chown "${user}:${user}" "/home/${user}/.hushlogin"
}

sys_services_enable() {
    chroot_run systemctl enable NetworkManager
    chroot_run systemctl enable fstrim.timer      2>/dev/null || true
    chroot_run systemctl enable reflector.timer   2>/dev/null || true
}

sys_install_os_release() {
    cat > /mnt/etc/os-release << 'EOF'
NAME="vendiOS"
PRETTY_NAME="vendiOS"
ID=vendios
ID_LIKE=arch
BUILD_ID=rolling
VERSION_CODENAME="zero"
ANSI_COLOR="38;2;203;166;247"
HOME_URL="https://github.com/vendi/vendiOS"
SUPPORT_URL="https://github.com/vendi/vendiOS/issues"
BUG_REPORT_URL="https://github.com/vendi/vendiOS/issues"
LOGO=vendios
EOF
}

sys_install_vendios_files() {
    mkdir -p /mnt/etc/vendios
    cat > /mnt/etc/vendios/release << EOF
VENDIOS_VERSION=0.1.0
VENDIOS_CODENAME=zero
VENDIOS_BUILD_DATE=$(date +%Y-%m-%d)
VENDIOS_ARCH=x86_64
VENDIOS_BASE=arch
EOF
    cat > /mnt/etc/vendios/config << 'EOF'
VENDIOS_AUR_HELPER=yay
VENDIOS_DEFAULT_SHELL=zsh
VENDIOS_ACCENT_COLOR="203;166;247"
VENDIOS_SNAPSHOT_MAX=10
EOF
    # copy the branding art (wordmark + shard logo)
    mkdir -p /mnt/usr/share/vendios
    for art in logo.txt shard.txt; do
        [[ -f /usr/share/vendios/$art ]] && \
            cp "/usr/share/vendios/$art" /mnt/usr/share/vendios/
    done
    # Default app associations (files/images/PDF/text/archives/media/web) so
    # double-clicking or `xdg-open` always lands in the matched GTK4 app.
    if [[ -f /etc/xdg/mimeapps.list ]]; then
        install -Dm644 /etc/xdg/mimeapps.list /mnt/etc/xdg/mimeapps.list
    fi
}

sys_install_vendi_cli() {
    # Mirror the future AUR `vendi` package layout (/usr/bin + /usr/lib/vendi)
    # so `yay -Syu` can manage updates to these paths after install.
    mkdir -p /mnt/usr/bin /mnt/usr/lib/vendi
    # Bash CLIs + the session launcher + the Rust binaries (vendiwm compositor,
    # vendi-ctl IPC, vendi-demo test client). All shipped from the live ISO's
    # /usr/bin since the airootfs is built with them in place.
    for bin in vendi vendi-install vendi-boot vendi-welcome vendi-session \
               vendiwm vendi-ctl vendi-demo vendibar vendi-menu vendi-launcher; do
        [[ -f /usr/bin/$bin ]] && install -m 755 /usr/bin/$bin /mnt/usr/bin/$bin
    done
    for lib in ui.sh disk.sh system.sh; do
        [[ -f /usr/lib/vendi/$lib ]] && \
            install -m 644 /usr/lib/vendi/$lib /mnt/usr/lib/vendi/$lib
    done

    # vendiOS desktop configs + assets, system-wide. Online installs also get
    # these from the vendi-git AUR package, but copying them here makes an
    # OFFLINE install complete — without it the quickshell bars report
    # "could not find config directory". These are clean config templates from
    # the live image (NO personal data — the live root has no user accounts).
    #   quickshell/  → vendibar-pro (notch) + vendilock configs
    #   waybar/      → Hyprland-fallback bar config
    #   *-portal*    → screen-share / portal routing for wlroots
    #   vendios/     → branding art (shard/logo)
    local tree
    for tree in /etc/xdg/quickshell /etc/xdg/waybar \
                /etc/xdg/xdg-desktop-portal-wlr /etc/xdg-desktop-portal \
                /usr/share/vendios; do
        [[ -d "$tree" ]] || continue
        mkdir -p "/mnt${tree}"
        cp -a "${tree}/." "/mnt${tree}/"
    done
}

sys_install_aur_helper() {
    # Build yay from AUR as the target user (makepkg refuses to run as root).
    # Then install vendi-git through yay with --overwrite so pacman takes
    # ownership of the files sys_install_vendi_cli pre-staged at /usr/bin
    # and /usr/lib/vendi (otherwise `yay -S vendi-git` later fails with
    # "file exists in filesystem").
    local user=$1

    # temporary NOPASSWD entry — removed when the function returns
    local sudoers_drop=/mnt/etc/sudoers.d/00-vendios-aur
    echo "${user} ALL=(ALL) NOPASSWD: ALL" > "$sudoers_drop"
    chmod 0440 "$sudoers_drop"

    arch-chroot /mnt sudo -u "$user" -H bash -c '
        set -e
        cd /tmp
        rm -rf yay-build
        git clone --depth=1 https://aur.archlinux.org/yay.git yay-build
        cd yay-build
        makepkg -si --noconfirm

        # Claim the pre-staged vendi files so future updates work via yay.
        # --overwrite tells pacman it is allowed to replace existing files.
        yay -S vendi-git --noconfirm \
            --mflags "--noconfirm" \
            --answerclean=N --answerdiff=N \
            --overwrite="/usr/bin/vendi*,/usr/lib/vendi/*,/usr/share/vendios/*,/etc/xdg/quickshell/*,/etc/xdg/waybar/*,/etc/xdg/xdg-desktop-portal-wlr/*"
    '
    local rc=$?

    rm -f "$sudoers_drop"
    return $rc
}

sys_fastfetch_config() {
    local user=$1
    mkdir -p "/mnt/home/${user}"
    local home="/mnt/home/${user}/.config/fastfetch"
    mkdir -p "$home"
    # Catppuccin Mocha theme with Nerd Font icons
    cat > "${home}/config.jsonc" << 'EOF'
{
  "$schema": "https://github.com/fastfetch-cli/fastfetch/raw/dev/doc/json_schema.json",
  "logo": {
    "type": "file",
    "source": "/usr/share/vendios/shard.txt",
    "padding": { "top": 1, "right": 4 }
  },
  "display": {
    "separator": "  ",
    "color": {
      "keys":   "38;2;203;166;247",
      "title":  "38;2;203;166;247",
      "output": "38;2;205;214;244"
    },
    "key": { "width": 12 }
  },
  "modules": [
    {
      "type": "title",
      "keyColor": "38;2;203;166;247"
    },
    { "type": "separator", "string": "─" },
    { "type": "os",       "key": "os",       "keyColor": "38;2;203;166;247" },
    { "type": "kernel",   "key": "kernel",   "keyColor": "38;2;180;190;254" },
    { "type": "uptime",   "key": "uptime",   "keyColor": "38;2;148;226;213" },
    { "type": "packages", "key": "packages", "keyColor": "38;2;166;227;161" },
    { "type": "shell",    "key": "shell",    "keyColor": "38;2;249;226;175" },
    { "type": "terminal", "key": "terminal", "keyColor": "38;2;250;179;135" },
    { "type": "cpu",      "key": "cpu",      "keyColor": "38;2;243;139;168" },
    { "type": "gpu",      "key": "gpu",      "keyColor": "38;2;235;160;172" },
    { "type": "memory",   "key": "memory",   "keyColor": "38;2;137;180;250" },
    { "type": "disk",     "key": "disk",     "keyColor": "38;2;108;112;134" },
    { "type": "separator", "string": "─" },
    { "type": "colors", "paddingLeft": 2, "symbol": "circle" }
  ]
}
EOF
    chroot_run chown -R "${user}:${user}" "/home/${user}/.config" 2>/dev/null || true
}

sys_zsh_config() {
    local user=$1
    local home="/mnt/home/${user}"
    mkdir -p "$home"

    cat > "${home}/.zshrc" << 'ZSHRC'
# vendiOS zsh — themed by `vendi theme` + Nerd Fonts
autoload -Uz compinit && compinit -d ~/.cache/zcompdump
autoload -Uz colors && colors

# ── palette ───────────────────────────────────────────────────
# Mocha defaults; `vendi theme` writes ~/.config/vendi/shell-theme.zsh
# which overrides every slot. RGB triplets for ANSI, hex for fzf/zstyle.
VENDI_ACCENT='203;166;247'    VENDI_ACCENT_HEX='#cba6f7'
VENDI_SECOND='180;190;254'
VENDI_DIM='108;112;134'
VENDI_ERR='243;139;168'       VENDI_ERR_HEX='#f38ba8'
VENDI_TEXT='205;214;244'      VENDI_TEXT_HEX='#cdd6f4'
VENDI_BASE_HEX='#1e1e2e'      VENDI_SURFACE_HEX='#313244'
VENDI_BLUE='137;180;250'
VENDI_TEAL='148;226;213'      VENDI_TEAL_HEX='#94e2d5'
VENDI_GREEN='166;227;161'     VENDI_GREEN_HEX='#a6e3a1'
VENDI_YELLOW='250;179;135'
VENDI_PINK_HEX='#f5c2e7'
[[ -f ~/.config/vendi/shell-theme.zsh ]] && source ~/.config/vendi/shell-theme.zsh

# ── prompt ────────────────────────────────────────────────────
_vendi_git_branch() {
    local branch
    branch=$(git symbolic-ref --short HEAD 2>/dev/null) || return
    local dirty=''
    git diff --quiet 2>/dev/null || dirty=' ●'
    printf '  %s%s' "$branch" "$dirty"   # nf-pl-branch
}

_vendi_prompt() {
    local code=$?
    local arrow_fg; arrow_fg=$( [[ $code -eq 0 ]] && echo "$VENDI_ACCENT" || echo "$VENDI_ERR" )
    # Pre-expand: PROMPT_SUBST expands $() in PS1 itself, but does NOT
    # re-expand command substitutions emitted from inside a function.
    local branch; branch=$(_vendi_git_branch)

    # segment 1: user
    printf '%b' "%{\e[38;2;${VENDI_ACCENT}m%}%B%n%b%f"
    # separator
    printf '%b' "%{\e[38;2;${VENDI_DIM}m%}  %f"    # nf-pl-right_soft_divider
    # segment 2: dir (shortened)
    printf '%b' "%{\e[38;2;${VENDI_SECOND}m%}%3~%f"
    # git
    printf '%b' "%{\e[38;2;${VENDI_DIM}m%}${branch}%f"
    # arrow
    printf '\n'
    printf '%b' "%{\e[38;2;${arrow_fg}m%} %f "    # nf-pl-right_hard_divider
}

setopt PROMPT_SUBST
PS1='$(_vendi_prompt)'
RPROMPT=''

# ── history ───────────────────────────────────────────────────
HISTSIZE=50000
SAVEHIST=50000
HISTFILE="${HOME}/.cache/zsh_history"
setopt HIST_IGNORE_ALL_DUPS HIST_IGNORE_SPACE SHARE_HISTORY INC_APPEND_HISTORY

# ── completions ───────────────────────────────────────────────
zstyle ':completion:*' menu select
zstyle ':completion:*' list-colors "${(s.:.)LS_COLORS}"
zstyle ':completion:*' matcher-list 'm:{a-z}={A-Z}' 'r:|=*' 'l:|=* r:|=*'
zstyle ':completion:*:descriptions' format "%F{${VENDI_ACCENT_HEX}}── %d ──%f"

# ── ls colors (Catppuccin) ────────────────────────────────────
export LS_COLORS="di=38;2;${VENDI_BLUE}:ln=38;2;${VENDI_TEAL}:ex=38;2;${VENDI_GREEN}:fi=38;2;${VENDI_TEXT}:*.zip=38;2;${VENDI_YELLOW}:*.tar=38;2;${VENDI_YELLOW}:*.gz=38;2;${VENDI_YELLOW}"

# ── aliases ───────────────────────────────────────────────────
# Modern replacements for classic tools, with classic-name aliases.
alias ls='eza --group-directories-first --icons=auto'
alias ll='eza -la --group-directories-first --icons=auto --git'
alias la='eza -a  --group-directories-first --icons=auto'
alias lt='eza -la --sort=modified --reverse --icons=auto'
alias tree='eza --tree --icons=auto'
alias cat='bat --paging=never --style=plain'
alias less='bat --paging=always'
alias grep='rg'
alias find='fd'
alias diff='diff --color=auto'
alias fetch='fastfetch'
alias update='vendi update'
alias rollback='vendi rollback'
alias cls='clear'

# ── fzf integration ───────────────────────────────────────────
if [[ -f /usr/share/fzf/key-bindings.zsh ]]; then
    source /usr/share/fzf/key-bindings.zsh
    source /usr/share/fzf/completion.zsh 2>/dev/null
    export FZF_DEFAULT_OPTS="--color=bg+:${VENDI_SURFACE_HEX},bg:${VENDI_BASE_HEX},fg:${VENDI_TEXT_HEX},fg+:${VENDI_TEXT_HEX},hl:${VENDI_ERR_HEX},hl+:${VENDI_ERR_HEX},info:${VENDI_ACCENT_HEX},marker:${VENDI_GREEN_HEX},prompt:${VENDI_ACCENT_HEX},pointer:${VENDI_PINK_HEX},spinner:${VENDI_TEAL_HEX},header:${VENDI_TEAL_HEX},border:${VENDI_ACCENT_HEX},gutter:${VENDI_BASE_HEX}"
fi
alias ..='cd ..'
alias ...='cd ../..'
alias mkdir='mkdir -p'

# ── environment ───────────────────────────────────────────────
export EDITOR=vim
export VISUAL=vim
export PAGER='less -R'
export MANPAGER='less -R'
export TERM=xterm-256color
ZSHRC

    cat > "${home}/.zprofile" << 'ZPRO'
[[ -f ~/.zshrc ]] && source ~/.zshrc
# Hyprland is launched directly by greetd's initial_session — no exec here.
ZPRO

    chroot_run chown "${user}:${user}" \
        "/home/${user}/.zshrc" \
        "/home/${user}/.zprofile" 2>/dev/null || true
}

sys_install_wm() {
    local user=$1
    local home="/mnt/home/${user}"

    # ── hyprland ──────────────────────────────────────────────
    local hcfg="${home}/.config/hypr"
    mkdir -p "$hcfg"
    cat > "${hcfg}/hyprland.conf" << 'EOF'
# vendiOS — Hyprland config (temp until vendiWM)
# Catppuccin Mocha

monitor = ,preferred,auto,1

# Hand off from Plymouth to Hyprland — splash stays visible until WM is up.
exec-once = plymouth quit --retain-splash
exec-once = waybar
exec-once = mako
exec-once = /usr/lib/polkit-kde-authentication-agent-1

$mod    = SUPER
$term   = foot
$menu   = wofi --show drun --allow-images --prompt ""

input {
    kb_layout    = us
    follow_mouse = 1
    sensitivity  = 0
    touchpad {
        natural_scroll    = true
        disable_while_typing = true
    }
}

general {
    gaps_in          = 5
    gaps_out         = 12
    border_size      = 2
    col.active_border   = rgb(CBA6F7) rgb(B4BEFE) 45deg
    col.inactive_border = rgb(313244)
    layout           = dwindle
    resize_on_border = true
}

decoration {
    rounding = 10
    blur {
        enabled  = true
        size     = 8
        passes   = 3
        noise    = 0.02
        contrast = 1.1
    }
    drop_shadow      = true
    shadow_range     = 18
    shadow_render_power = 3
    col.shadow       = rgba(1e1e2ecc)
    col.shadow_inactive = rgba(1e1e2e66)
    dim_inactive     = true
    dim_strength     = 0.08
}

animations {
    enabled = true
    bezier  = snap,   0.25, 1.0,  0.5,  1.0
    bezier  = slide,  0.4,  0.0,  0.2,  1.0
    bezier  = expo,   0.16, 1.0,  0.3,  1.0

    animation = windows,       1, 4,  snap,  slide
    animation = windowsOut,    1, 3,  expo,  popin 80%
    animation = border,        1, 8,  slide
    animation = borderangle,   1, 80, slide, loop
    animation = fade,          1, 4,  expo
    animation = workspaces,    1, 4,  expo,  slide
}

dwindle {
    pseudotile      = false
    preserve_split  = true
    smart_split     = true
}

gestures {
    workspace_swipe = true
    workspace_swipe_fingers = 3
}

misc {
    force_default_wallpaper = 0
    disable_hyprland_logo   = true
    disable_splash_rendering = true
    background_color        = rgb(1e1e2e)
}

# ── keybinds ──────────────────────────────────────────────────
bind = $mod,       Return,     exec,        $term
bind = $mod,       Space,      exec,        $menu
bind = $mod,       Q,          killactive
bind = $mod SHIFT, E,          exit
bind = $mod,       F,          fullscreen
bind = $mod,       V,          togglefloating
bind = $mod,       P,          pseudo
bind = $mod,       J,          togglesplit
bind = $mod,       L,          exec,        swaylock

# focus
bind = $mod, left,  movefocus, l
bind = $mod, right, movefocus, r
bind = $mod, up,    movefocus, u
bind = $mod, down,  movefocus, d
bind = $mod, H,     movefocus, l
bind = $mod, L,     movefocus, r
bind = $mod, K,     movefocus, u
bind = $mod, J,     movefocus, d

# workspaces
bind = $mod,       1, workspace, 1
bind = $mod,       2, workspace, 2
bind = $mod,       3, workspace, 3
bind = $mod,       4, workspace, 4
bind = $mod,       5, workspace, 5
bind = $mod SHIFT, 1, movetoworkspace, 1
bind = $mod SHIFT, 2, movetoworkspace, 2
bind = $mod SHIFT, 3, movetoworkspace, 3
bind = $mod SHIFT, 4, movetoworkspace, 4
bind = $mod SHIFT, 5, movetoworkspace, 5

# screenshot
bind  = , Print,          exec, grim ~/Screenshots/$(date +%s).png
bind  = SHIFT, Print,     exec, grim -g "$(slurp)" ~/Screenshots/$(date +%s).png

# brightness / volume
bindel = ,XF86MonBrightnessUp,   exec, brightnessctl set +5%
bindel = ,XF86MonBrightnessDown, exec, brightnessctl set 5%-
bindel = ,XF86AudioRaiseVolume,  exec, wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+
bindel = ,XF86AudioLowerVolume,  exec, wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-
bindl  = ,XF86AudioMute,         exec, wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle

# mouse
bindm = $mod, mouse:272, movewindow
bindm = $mod, mouse:273, resizewindow

# window rules
windowrulev2 = float,        class:(wofi)
windowrulev2 = center,       class:(wofi)
windowrulev2 = float,        class:(mako)
windowrulev2 = animation popin, class:(wofi)
EOF

    # ── waybar ────────────────────────────────────────────────
    local wcfg="${home}/.config/waybar"
    mkdir -p "$wcfg"
    cat > "${wcfg}/config.jsonc" << 'EOF'
{
  "layer": "top",
  "position": "top",
  "height": 36,
  "spacing": 4,
  "modules-left":   ["hyprland/workspaces", "hyprland/window"],
  "modules-center": ["clock"],
  "modules-right":  ["pulseaudio", "network", "memory", "cpu", "battery", "tray"],

  "hyprland/workspaces": {
    "format": "{icon}",
    "format-icons": {
      "1": "󰲡", "2": "󰲣", "3": "󰲥", "4": "󰲧", "5": "󰲩",
      "active": "󰮯", "default": "󰊠"
    },
    "persistent-workspaces": { "*": 5 }
  },
  "hyprland/window": { "max-length": 50, "separate-outputs": true },
  "clock": {
    "format": "  {:%H:%M}",
    "format-alt": "󰸗  {:%a %d %b}",
    "tooltip-format": "<tt>{calendar}</tt>"
  },
  "cpu":    { "format": "  {usage}%", "interval": 2 },
  "memory": { "format": "  {used:.1f}G", "interval": 4 },
  "network": {
    "format-wifi":         "  {essid}",
    "format-ethernet":     "󰈀  {ipaddr}",
    "format-disconnected": "󰤭  offline",
    "tooltip-format-wifi": "{signalStrength}%"
  },
  "pulseaudio": {
    "format":        "{icon}  {volume}%",
    "format-muted":  "󰝟  muted",
    "format-icons":  { "default": ["󰕿", "󰖀", "󰕾"] },
    "on-click":      "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
  },
  "battery": {
    "format":          "{icon}  {capacity}%",
    "format-charging": "󰂄  {capacity}%",
    "format-icons":    ["󰂎","󰁺","󰁻","󰁼","󰁽","󰁾","󰁿","󰂀","󰂁","󰂂","󰁹"]
  },
  "tray": { "spacing": 8 }
}
EOF

    cat > "${wcfg}/style.css" << 'EOF'
/* vendiOS waybar — Catppuccin Mocha */
* { font-family: "JetBrainsMono Nerd Font", monospace; font-size: 13px; border: none; }

window#waybar {
    background: rgba(17,17,27,0.92);
    color: #CDD6F4;
    border-bottom: 1px solid rgba(203,166,247,0.2);
}

.modules-left, .modules-center, .modules-right { padding: 0 8px; }

#workspaces button {
    color: #6C7086;
    padding: 0 6px;
    border-radius: 6px;
    transition: all 0.2s;
}
#workspaces button.active {
    color: #CBA6F7;
    background: rgba(203,166,247,0.15);
}
#workspaces button:hover {
    color: #CDD6F4;
    background: rgba(255,255,255,0.08);
}

#window      { color: #A6ADC8; }
#clock       { color: #B4BEFE; font-weight: bold; }
#cpu         { color: #F38BA8; }
#memory      { color: #89B4FA; }
#network     { color: #94E2D5; }
#pulseaudio  { color: #A6E3A1; }
#battery     { color: #F9E2AF; }
#battery.charging { color: #A6E3A1; }
#tray        { color: #CDD6F4; }

tooltip {
    background: #1E1E2E;
    color: #CDD6F4;
    border: 1px solid rgba(203,166,247,0.4);
    border-radius: 8px;
}
EOF

    # ── mako (notifications) ──────────────────────────────────
    local mcfg="${home}/.config/mako"
    mkdir -p "$mcfg"
    cat > "${mcfg}/config" << 'EOF'
background-color=#1E1E2E
text-color=#CDD6F4
border-color=#CBA6F7
border-size=1
border-radius=8
width=320
height=120
margin=12
padding=12,14
font=JetBrainsMono Nerd Font 11
default-timeout=5000

[urgency=high]
border-color=#F38BA8
EOF

    # ── foot (terminal) ───────────────────────────────────────
    local fcfg="${home}/.config/foot"
    mkdir -p "$fcfg"
    cat > "${fcfg}/foot.ini" << 'EOF'
[main]
font=JetBrainsMono Nerd Font:size=12
pad=12x10

[colors]
background=1e1e2e
foreground=cdd6f4
regular0=45475a
regular1=f38ba8
regular2=a6e3a1
regular3=f9e2af
regular4=89b4fa
regular5=f5c2e7
regular6=94e2d5
regular7=bac2de
bright0=585b70
bright1=f38ba8
bright2=a6e3a1
bright3=f9e2af
bright4=89b4fa
bright5=f5c2e7
bright6=94e2d5
bright7=a6adc8
selection-background=313244
selection-foreground=cdd6f4

[cursor]
text-color=1e1e2e
cursor-color=cba6f7

[mouse]
hide-when-typing=yes
EOF

    # ── swaylock (lock screen, Mocha) ─────────────────────────
    local slcfg="${home}/.config/swaylock"
    mkdir -p "$slcfg"
    cat > "${slcfg}/config" << 'EOF'
ignore-empty-password
indicator-radius=90
indicator-thickness=10
color=11111b
inside-color=1e1e2e
inside-clear-color=1e1e2e
inside-ver-color=1e1e2e
inside-wrong-color=1e1e2e
ring-color=45475a
ring-clear-color=f9e2af
ring-ver-color=cba6f7
ring-wrong-color=f38ba8
line-color=00000000
line-clear-color=00000000
line-ver-color=00000000
line-wrong-color=00000000
separator-color=00000000
key-hl-color=cba6f7
bs-hl-color=f38ba8
text-color=cdd6f4
text-clear-color=cdd6f4
text-ver-color=cdd6f4
text-wrong-color=f38ba8
font=JetBrainsMono Nerd Font
EOF

    # ── wofi ─────────────────────────────────────────────────
    local wfcfg="${home}/.config/wofi"
    mkdir -p "$wfcfg"
    cat > "${wfcfg}/style.css" << 'EOF'
* { font-family: "JetBrainsMono Nerd Font"; font-size: 13px; }

window {
    background: rgba(30,30,46,0.96);
    border: 1px solid rgba(203,166,247,0.4);
    border-radius: 12px;
}
#input {
    background: #181825;
    color: #CDD6F4;
    border: none;
    border-bottom: 1px solid rgba(203,166,247,0.3);
    border-radius: 12px 12px 0 0;
    padding: 12px 16px;
    font-size: 14px;
}
#inner-box    { background: transparent; margin: 6px; }
#outer-box    { background: transparent; }
#scroll       { background: transparent; }
#entry        { padding: 8px 12px; border-radius: 8px; color: #CDD6F4; }
#entry:focus  { background: rgba(203,166,247,0.12); color: #CBA6F7; }
#entry:hover  { background: rgba(255,255,255,0.05); }
#text         { color: #CDD6F4; }
#text:focus   { color: #CBA6F7; }
EOF

    # ── screenshots dir ───────────────────────────────────────
    chroot_run mkdir -p "/home/${user}/Screenshots"
    chroot_run chown -R "${user}:${user}" "/home/${user}" 2>/dev/null || true

    # ── enable pipewire ───────────────────────────────────────
    chroot_run systemctl enable --global pipewire pipewire-pulse wireplumber 2>/dev/null || true
}
