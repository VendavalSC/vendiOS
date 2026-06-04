# vendiOS

> Minimal. Modern. Yours.

vendiOS is an Arch-based Linux distribution with a custom TUI installer, Catppuccin
Mocha theming throughout, and Limine for boot. The goal is a fast, opinionated
desktop that starts working the moment you finish installing it.

**Status:** Early development. Not yet recommended for daily-driver use.

---

## What's in the box

- **Custom TUI installer** — Catppuccin Mocha, font-independent panel chrome,
  proportional disk visualization, searchable timezone/keymap pickers.
- **Limine bootloader** — same loader for the live ISO and the installed system.
  FAT32 `/boot` partition (the only filesystem Limine reads on BIOS).
- **btrfs by default** — subvolume layout (`@`, `@home`, `@var`, `@snapshots`)
  with zstd compression. ext4 also supported.
- **Hyprland desktop** — Wayland-only, configured to launch on first login.
- **`vendi` CLI** — one command for updates, snapshots, clean-up, and info.

## Repository layout

```
vendiOS/
├── archiso/                      # mkarchiso profile
│   ├── airootfs/                 # files copied into the live ISO
│   │   ├── usr/bin/              # vendi, vendi-install, vendi-boot
│   │   ├── usr/lib/vendi/        # ui.sh, disk.sh, system.sh
│   │   └── usr/share/vendios/    # branding
│   ├── packages.x86_64           # packages pacstrapped into the live ISO
│   └── profiledef.sh             # ISO metadata
├── pkg/
│   ├── vendi-git/                # rolling AUR package (tracks main)
│   └── vendi/                    # stable AUR package (tagged releases)
└── build.sh                      # ISO builder
```

## Building the ISO

Requires Arch (or an Arch-based distro) with `archiso`, `limine`, and `xorriso`
installed.

```bash
sudo pacman -S archiso limine xorriso
sudo bash build.sh --clean
```

The resulting ISO is written to `out/vendios-YYYY.MM.DD-x86_64.iso`. Boot it in
QEMU, write it to a USB stick with `dd`, or flash it however you prefer.

```bash
# Quick QEMU test (BIOS)
qemu-system-x86_64 \
    -enable-kvm -m 4G -smp 2 \
    -drive file=out/vendios-*.iso,media=cdrom,readonly=on \
    -drive file=test-disk.img,if=virtio,format=raw \
    -boot d
```

## Installing the `vendi` CLI on an existing Arch system

Once published to AUR:

```bash
paru -S vendi-git        # rolling
# or
paru -S vendi            # stable
```

Then `pacman -Syu` / `paru -Syu` handles updates like any other package.

## Updating an installed vendiOS

```bash
sudo vendi update        # refreshes mirrors and runs pacman -Syu
```

## License

MIT
