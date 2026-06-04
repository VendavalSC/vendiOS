#!/usr/bin/env bash
# Runs inside the chroot after pacstrap during ISO build.

set -euo pipefail

# Initialize pacman keyring in the live env. Normally archlinux-keyring's
# post-install hook handles this, but it's not guaranteed under archiso
# build conditions — be explicit so `pacstrap -K` during install can copy
# a known-good keyring into the target.
pacman-key --init
pacman-key --populate archlinux

locale-gen

# services enabled in the live environment
systemctl enable NetworkManager
systemctl enable systemd-resolved
systemctl enable sshd

# unlock root for live env (empty password — installer auto-launches via .bash_profile)
passwd -d root

# suppress kernel ring buffer spam on console
sysctl -w kernel.printk="0 4 1 7" 2>/dev/null || true

# sudoers: wheel group passwordless in live env
sed -i 's/^# %wheel ALL=(ALL:ALL) NOPASSWD: ALL/%wheel ALL=(ALL:ALL) NOPASSWD: ALL/' /etc/sudoers

# set UTC for live env
ln -sf /usr/share/zoneinfo/UTC /etc/localtime

# build font cache so foot can find JetBrains Mono Nerd Font on first boot
fc-cache -f 2>/dev/null || true

# clean up
rm -f /etc/machine-id
