#!/usr/bin/env bash
# vendiOS disk library

FIRMWARE='bios'
[[ -d /sys/firmware/efi/efivars ]] && FIRMWARE='uefi'

# ── disk enumeration ──────────────────────────────────────────────────────────
disk_list() {
    lsblk -dno NAME,SIZE,MODEL,TYPE 2>/dev/null > /tmp/lsblk.log
    local name size model
    while read -r name; do
        [[ -z "$name" ]] && continue
        size=$(lsblk -dno SIZE "/dev/${name}" 2>/dev/null)
        model=$(lsblk -dno MODEL "/dev/${name}" 2>/dev/null | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        [[ -z "$model" ]] && model="Unknown"
        echo "/dev/${name}|${size}|${model}"
    done < <(lsblk -dno NAME,TYPE 2>/dev/null | awk '$NF=="disk" {print $1}')
}

disk_is_nvme() { [[ "$1" == *nvme* || "$1" == *mmcblk* ]]; }
disk_part()    { disk_is_nvme "$1" && echo "${1}p${2}" || echo "${1}${2}"; }

# ── free space detection ──────────────────────────────────────────────────────
disk_largest_free() {
    local disk=$1
    parted -s "$disk" unit MiB print free 2>/dev/null | \
    awk '/Free Space/ {
        gsub(/MiB/,"")
        size = $3 - $1
        if (size > max) { max=size; start=$1; end=$3 }
    } END {
        if (max > 0) print start, end, max
    }'
}

disk_free_ok() {
    local result; result=$(disk_largest_free "$1")
    local size_mib; size_mib=$(echo "$result" | awk '{print $3}')
    [[ -n "$size_mib" && "$size_mib" -ge 8192 ]]
}

disk_find_esp() {
    local disk=$1
    lsblk -no NAME,PARTTYPE "$disk" 2>/dev/null | \
        awk '/c12a7328-f81f-11d2-ba4b-00a0c93ec93b/{print "/dev/"$1}' | head -1
}

# ── partitioning ──────────────────────────────────────────────────────────────
# IMPORTANT: limine only reads FAT32 (and ISO9660). The kernel, initramfs,
# limine.conf, and limine-bios.sys must live on a FAT32 partition.
# Layout for both BIOS and UEFI: p1 = FAT32 /boot (513 MiB), p2 = root.

disk_wipe() {
    wipefs -af "$1" >/dev/null 2>&1
    sgdisk --zap-all "$1" >/dev/null 2>&1
}

disk_partition_full_uefi() {
    local disk=$1
    parted -s "$disk" mklabel gpt \
        mkpart EFI  fat32  1MiB  513MiB set 1 esp on \
        mkpart ROOT       513MiB 100%
}

disk_partition_full_bios() {
    local disk=$1
    # Two partitions: FAT32 /boot (513 MiB, bootable) + root.
    # Limine BIOS stage1 in MBR reads limine-bios.sys from the FAT32 partition.
    parted -s "$disk" mklabel msdos \
        mkpart primary fat32  1MiB  513MiB set 1 boot on \
        mkpart primary       513MiB 100%
}

# MODE: install into largest free space (UEFI only — BIOS dual-boot is not
# supported because we'd need to rearrange the existing MBR layout).
disk_partition_free_space() {
    local disk=$1
    local free; free=$(disk_largest_free "$disk")
    local start end size
    read -r start end size <<< "$free"

    if [[ "$FIRMWARE" != 'uefi' ]]; then
        echo ""
        return 1
    fi

    local existing_esp; existing_esp=$(disk_find_esp "$disk")
    if [[ -n "$existing_esp" ]]; then
        parted -s "$disk" mkpart ROOT "${start}MiB" "${end}MiB"
        partprobe "$disk"; sleep 1
        local root_part; root_part=$(lsblk -lno NAME "$disk" | tail -1 | sed 's|^|/dev/|')
        echo "$existing_esp $root_part"
    else
        local efi_end=$(( start + 513 ))
        [[ $efi_end -ge $end ]] && { echo ""; return 1; }
        parted -s "$disk" \
            mkpart EFI fat32 "${start}MiB" "${efi_end}MiB" set "$(disk_next_partnum "$disk")" esp on \
            mkpart ROOT "${efi_end}MiB" "${end}MiB"
        partprobe "$disk"; sleep 1
        local parts=(); while IFS= read -r p; do parts+=("$p"); done < <(lsblk -lno NAME "$disk" | tail -2 | sed 's|^|/dev/|')
        echo "${parts[0]} ${parts[1]}"
    fi
}

disk_next_partnum() {
    local disk=$1
    lsblk -lno NAME "$disk" | grep -v "^${disk##*/}$" | wc -l
}

# ── formatting ────────────────────────────────────────────────────────────────
disk_format_boot()       { mkfs.fat -F32 -n VENDIBOOT "$1" >/dev/null 2>&1; }
disk_format_root_btrfs() { mkfs.btrfs -f -L vendios "$1" >/dev/null 2>&1; }
disk_format_root_ext4()  { mkfs.ext4  -F -L vendios "$1" >/dev/null 2>&1; }

# ── LUKS encryption ───────────────────────────────────────────────────────────
# disk_luks_format <partition> <passphrase>  → creates LUKS container
# disk_luks_open   <partition> <passphrase>  → opens at /dev/mapper/cryptroot
# disk_luks_uuid   <partition>               → UUID of the LUKS header
disk_luks_format() {
    local part=$1 pass=$2
    printf '%s' "$pass" | cryptsetup --batch-mode --type luks2 \
        --cipher aes-xts-plain64 --key-size 512 --hash sha512 \
        --pbkdf argon2id luksFormat "$part" -
}

disk_luks_open() {
    local part=$1 pass=$2
    printf '%s' "$pass" | cryptsetup --batch-mode open "$part" cryptroot -
}

disk_luks_uuid() { blkid -s UUID -o value "$1" 2>/dev/null; }

# Legacy alias — used by older code paths
disk_format_efi() { disk_format_boot "$1"; }

# ── mounting ──────────────────────────────────────────────────────────────────
# Both BIOS and UEFI: boot partition mounts at /mnt/boot. The kernel installed
# by pacstrap lands directly on the FAT32 partition where Limine can read it.

disk_mount_btrfs() {
    local root=$1 boot=${2:-}
    {
        mount "$root" /mnt
        btrfs subvolume create /mnt/@
        btrfs subvolume create /mnt/@home
        btrfs subvolume create /mnt/@var
        btrfs subvolume create /mnt/@snapshots
        umount /mnt

        local opts='noatime,compress=zstd:1,space_cache=v2'
        mount -o "${opts},subvol=@"          "$root" /mnt
        mkdir -p /mnt/{home,var,.snapshots,boot}
        mount -o "${opts},subvol=@home"      "$root" /mnt/home
        mount -o "${opts},subvol=@var"       "$root" /mnt/var
        mount -o "${opts},subvol=@snapshots" "$root" /mnt/.snapshots
        if [[ -n "$boot" ]]; then
            mount "$boot" /mnt/boot
        fi
    } >/dev/null 2>&1
}

disk_mount_ext4() {
    local root=$1 boot=${2:-}
    {
        mount "$root" /mnt
        if [[ -n "$boot" ]]; then
            mkdir -p /mnt/boot
            mount "$boot" /mnt/boot
        fi
    } >/dev/null 2>&1
}

# ── limine install ────────────────────────────────────────────────────────────
# Unified: kernel + initramfs + limine.conf + (limine-bios.sys on BIOS) all
# live on the FAT32 /boot partition. Pacstrap put the kernel there already.

limine_install() {
    # Args: <disk> <firmware> <root_partuuid> <fs> [luks_uuid]
    # When luks_uuid is set, the cmdline uses cryptdevice= + /dev/mapper/cryptroot
    # so plymouth-encrypt unlocks the root before mounting.
    local disk=$1 firmware=$2 root_partuuid=$3 fs=$4 luks_uuid=${5:-}
    local cmdline=''

    if [[ -n "$luks_uuid" ]]; then
        cmdline="cryptdevice=UUID=${luks_uuid}:cryptroot root=/dev/mapper/cryptroot"
    else
        cmdline="root=PARTUUID=${root_partuuid}"
    fi
    cmdline+=" rw quiet loglevel=3"                            # only warnings+errors
    cmdline+=" rd.systemd.show_status=false rd.udev.log_level=0"  # silence udev in initramfs
    cmdline+=" systemd.show_status=false udev.log_level=0"
    cmdline+=" vt.global_cursor_default=0 fbcon=nodefer"        # no cursor, splash unhidden
    cmdline+=" splash bgrt_disable"                              # Plymouth, no UEFI bgrt
    [[ "$fs" == 'btrfs' ]] && cmdline+=' rootflags=subvol=@'

    local boot=/mnt/boot

    # write limine.conf — same format for both BIOS and UEFI
    {
        echo "timeout: 0"
        echo "quiet: yes"      # suppress Limine's "loading kernel…" boot chatter
        echo "serial: no"
        echo ""
        echo "/vendiOS"
        echo "    protocol: linux"
        echo "    kernel_path: boot():/vmlinuz-linux"
        echo "    cmdline: ${cmdline}"
        echo "    module_path: boot():/initramfs-linux.img"
        [[ -f "${boot}/amd-ucode.img"   ]] && echo "    module_path: boot():/amd-ucode.img"
        [[ -f "${boot}/intel-ucode.img" ]] && echo "    module_path: boot():/intel-ucode.img"
    } > "${boot}/limine.conf"

    if [[ "$firmware" == 'uefi' ]]; then
        mkdir -p "${boot}/EFI/BOOT"
        cp /usr/share/limine/BOOTX64.EFI   "${boot}/EFI/BOOT/"  2>/dev/null || true
        cp /usr/share/limine/BOOTIA32.EFI  "${boot}/EFI/BOOT/"  2>/dev/null || true

        # register EFI boot entry
        local dev; dev=$(findmnt -no SOURCE "$boot")
        local partnum; partnum=$(cat "/sys/class/block/${dev##*/}/partition" 2>/dev/null || echo 1)
        efibootmgr --disk "$disk" --part "$partnum" \
            --create --label "vendiOS" \
            --loader '/EFI/BOOT/BOOTX64.EFI' >/dev/null 2>&1 || true
    else
        cp /usr/share/limine/limine-bios.sys "${boot}/"
        sync
        # Stage 1 MBR install — patches MBR with pointers to limine-bios.sys
        # which it locates by scanning partitions for the FAT32 we just wrote.
        limine bios-install "$disk"
    fi
}

disk_partuuid() { blkid -s PARTUUID -o value "$1" 2>/dev/null; }
