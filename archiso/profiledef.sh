#!/usr/bin/env bash

iso_name="vendios"
iso_label="VENDIOS_$(date +%Y%m)"
iso_publisher="vendiOS <https://github.com/vendi/vendiOS>"
iso_application="vendiOS"
iso_version="$(date +%Y.%m.%d)"
install_dir="arch"
buildmodes=('iso')
bootmodes=(
  'bios.syslinux'
  'uefi.systemd-boot'
)
arch="x86_64"
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '19' '-b' '1M')
bootstrap_tarball_compression=('zstd' '-c' '-T0' '--ultra' '-20' '-')

file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/etc/gshadow"]="0:0:400"
  ["/root"]="0:0:750"
  ["/root/customize_airootfs.sh"]="0:0:755"
  ["/usr/bin/vendi-boot"]="0:0:755"
  ["/usr/bin/vendi-install"]="0:0:755"
  ["/usr/bin/vendi"]="0:0:755"
  ["/usr/bin/vendi-welcome"]="0:0:755"
  ["/usr/bin/vendi-session"]="0:0:755"
  ["/usr/bin/vendiwm"]="0:0:755"
  ["/usr/bin/vendi-ctl"]="0:0:755"
  ["/usr/bin/vendi-demo"]="0:0:755"
  ["/usr/bin/vendibar"]="0:0:755"
  ["/usr/bin/vendi-menu"]="0:0:755"
  ["/usr/lib/vendi/ui.sh"]="0:0:644"
  ["/usr/lib/vendi/disk.sh"]="0:0:644"
  ["/usr/lib/vendi/system.sh"]="0:0:644"
  ["/root/.bash_profile"]="0:0:644"
  ["/root/.config/hypr/hyprland.conf"]="0:0:644"
  ["/root/.config/alacritty/alacritty.toml"]="0:0:644"
)
